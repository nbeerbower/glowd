//! MagicHome (Zengge) LAN protocol.
//!
//! Discovery: UDP broadcast "HF-A11ASSISTHREAD" to port 48899; devices reply
//! with "ip,mac,model". Control: TCP port 5577, each command is a few bytes
//! followed by a checksum (sum of all bytes, truncated to u8).

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use serde::Serialize;

const CONTROL_PORT: u16 = 5577;
const DISCOVERY_PORT: u16 = 48899;
const DISCOVERY_MSG: &[u8] = b"HF-A11ASSISTHREAD";
const IO_TIMEOUT: Duration = Duration::from_secs(3);

/// Built-in animation effects: (name, mode byte).
pub const EFFECTS: &[(&str, u8)] = &[
    ("seven_color_cross_fade", 0x25),
    ("red_gradual", 0x26),
    ("green_gradual", 0x27),
    ("blue_gradual", 0x28),
    ("yellow_gradual", 0x29),
    ("cyan_gradual", 0x2A),
    ("purple_gradual", 0x2B),
    ("white_gradual", 0x2C),
    ("red_green_cross", 0x2D),
    ("red_blue_cross", 0x2E),
    ("green_blue_cross", 0x2F),
    ("seven_color_strobe", 0x30),
    ("red_strobe", 0x31),
    ("green_strobe", 0x32),
    ("blue_strobe", 0x33),
    ("yellow_strobe", 0x34),
    ("cyan_strobe", 0x35),
    ("purple_strobe", 0x36),
    ("white_strobe", 0x37),
    ("seven_color_jump", 0x38),
];

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub ip: String,
    pub mac: String,
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct State {
    pub on: bool,
    /// "color" for static color, an effect name, or "0x??" for unknown modes.
    pub mode: String,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// Raw protocol speed byte (0x01 fastest .. 0x1F slowest).
    pub speed: u8,
}

fn with_checksum(cmd: &[u8]) -> Vec<u8> {
    let sum: u32 = cmd.iter().map(|&b| u32::from(b)).sum();
    let mut msg = cmd.to_vec();
    msg.push((sum & 0xFF) as u8);
    msg
}

fn send(ip: &str, cmd: &[u8], reply_len: usize) -> io::Result<Vec<u8>> {
    let addr: SocketAddr = format!("{ip}:{CONTROL_PORT}")
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad ip address"))?;
    let mut stream = TcpStream::connect_timeout(&addr, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(&with_checksum(cmd))?;
    let mut reply = vec![0u8; reply_len];
    if reply_len > 0 {
        stream.read_exact(&mut reply)?;
    }
    Ok(reply)
}

/// Parse a discovery reply of the form "ip,mac,model".
fn parse_discovery_reply(reply: &str) -> Option<Device> {
    let parts: Vec<&str> = reply.trim().split(',').collect();
    let [ip, mac, model] = parts[..] else {
        return None;
    };
    Some(Device {
        ip: ip.to_string(),
        mac: mac.to_string(),
        model: model.to_string(),
    })
}

/// Broadcast a discovery probe and collect replies until `timeout` elapses.
pub fn discover(timeout: Duration) -> io::Result<Vec<Device>> {
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_broadcast(true)?;
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    sock.send_to(DISCOVERY_MSG, ("255.255.255.255", DISCOVERY_PORT))?;

    let deadline = Instant::now() + timeout;
    let mut devices: Vec<Device> = Vec::new();
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        let Ok((n, _)) = sock.recv_from(&mut buf) else {
            continue; // read timeout tick; keep waiting until the deadline
        };
        if &buf[..n] == DISCOVERY_MSG {
            continue; // our own broadcast echoed back
        }
        let reply = String::from_utf8_lossy(&buf[..n]);
        if let Some(device) = parse_discovery_reply(&reply) {
            if !devices.iter().any(|d| d.ip == device.ip) {
                devices.push(device);
            }
        }
    }
    Ok(devices)
}

/// Decode the 14-byte reply to a state query.
fn parse_state(resp: &[u8]) -> State {
    let mode = match resp[3] {
        0x61 => "color".to_string(),
        byte => EFFECTS
            .iter()
            .find(|(_, code)| *code == byte)
            .map(|(name, _)| name.to_string())
            .unwrap_or_else(|| format!("0x{byte:02x}")),
    };
    State {
        on: resp[2] == 0x23,
        mode,
        r: resp[6],
        g: resp[7],
        b: resp[8],
        speed: resp[5],
    }
}

pub fn state(ip: &str) -> io::Result<State> {
    let resp = send(ip, &[0x81, 0x8A, 0x8B], 14)?;
    Ok(parse_state(&resp))
}

pub fn set_power(ip: &str, on: bool) -> io::Result<()> {
    send(ip, &[0x71, if on { 0x23 } else { 0x24 }, 0x0F], 0)?;
    Ok(())
}

pub fn set_color(ip: &str, r: u8, g: u8, b: u8) -> io::Result<()> {
    send(ip, &[0x31, r, g, b, 0x00, 0xF0, 0x0F], 0)?;
    Ok(())
}

/// Map a 1-100 percentage (fastest = 100) onto the protocol's speed byte
/// (0x01 fastest, 0x1F slowest).
fn speed_byte(speed_pct: u8) -> u8 {
    let pct = u32::from(speed_pct.clamp(1, 100));
    (0x1F - (pct * 0x1E) / 100) as u8
}

/// `speed_pct`: 1 (slowest) .. 100 (fastest).
pub fn set_effect(ip: &str, name: &str, speed_pct: u8) -> io::Result<()> {
    let (_, code) = EFFECTS
        .iter()
        .find(|(n, _)| *n == name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unknown effect"))?;
    send(ip, &[0x61, *code, speed_byte(speed_pct), 0x0F], 0)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_truncated_byte_sum() {
        // 0x81 + 0x8A + 0x8B = 0x196 -> 0x96
        assert_eq!(
            with_checksum(&[0x81, 0x8A, 0x8B]),
            vec![0x81, 0x8A, 0x8B, 0x96]
        );
        assert_eq!(with_checksum(&[]), vec![0x00]);
    }

    #[test]
    fn parses_real_state_reply() {
        // Captured from an AK001-ZJ200: on, static color (255, 17, 0).
        let resp = [
            0x81, 0x33, 0x23, 0x61, 0x01, 0x10, 0xFF, 0x11, 0x00, 0x00, 0x04, 0x00, 0x00, 0x5D,
        ];
        let st = parse_state(&resp);
        assert!(st.on);
        assert_eq!(st.mode, "color");
        assert_eq!((st.r, st.g, st.b), (255, 17, 0));
        assert_eq!(st.speed, 16);
    }

    #[test]
    fn parses_effect_and_off_states() {
        let mut resp = [0u8; 14];
        resp[2] = 0x24; // off
        resp[3] = 0x25; // seven_color_cross_fade
        let st = parse_state(&resp);
        assert!(!st.on);
        assert_eq!(st.mode, "seven_color_cross_fade");

        resp[3] = 0x99; // unknown mode byte
        assert_eq!(parse_state(&resp).mode, "0x99");
    }

    #[test]
    fn parses_discovery_replies() {
        let dev = parse_discovery_reply("192.168.1.102,60019496E1B0,AK001-ZJ200").unwrap();
        assert_eq!(dev.ip, "192.168.1.102");
        assert_eq!(dev.mac, "60019496E1B0");
        assert_eq!(dev.model, "AK001-ZJ200");
        assert!(parse_discovery_reply("garbage").is_none());
        assert!(parse_discovery_reply("a,b,c,d").is_none());
    }

    #[test]
    fn speed_byte_covers_full_protocol_range() {
        assert_eq!(speed_byte(100), 0x01); // fastest
        assert_eq!(speed_byte(1), 0x1F); // slowest
        assert_eq!(speed_byte(0), 0x1F); // clamped up
        assert_eq!(speed_byte(255), 0x01); // clamped down
    }
}
