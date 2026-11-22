use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

pub const MAX_LINE: usize = 4 * 1024 * 1024;

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum Incoming {
    #[serde(rename = "notice.show", rename_all = "camelCase")]
    NoticeShow {
        notice_id: String,
        message: String,
        remaining_seconds: u64,
    },
    #[serde(rename = "notice.hide", rename_all = "camelCase")]
    NoticeHide { notice_id: String },
    #[serde(rename = "preview.start", rename_all = "camelCase")]
    PreviewStart {
        max_width: u32,
        fps: u32,
        #[serde(default = "default_show_indicator")]
        show_indicator: bool,
    },
    #[serde(rename = "preview.stop")]
    PreviewStop,
}

fn default_show_indicator() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum Outgoing<'a> {
    #[serde(rename = "hello", rename_all = "camelCase")]
    Hello {
        session_id: u32,
        pid: u32,
        version: &'a str,
    },
    #[serde(rename = "notice.displayed", rename_all = "camelCase")]
    NoticeDisplayed { notice_id: &'a str },
    #[serde(rename = "notice.failed", rename_all = "camelCase")]
    NoticeFailed { notice_id: &'a str, reason: &'a str },
    #[serde(rename = "preview.frame", rename_all = "camelCase")]
    PreviewFrame {
        jpeg_base64: &'a str,
        width: u32,
        height: u32,
        captured_at_utc: &'a str,
    },
    #[serde(rename = "preview.failed")]
    PreviewFailed { reason: &'a str },
}

// fill_buf avoids allocating beyond the hard cap even for a peer that never sends '\n'.
pub fn read_line<R: BufRead>(reader: &mut R) -> io::Result<Option<Incoming>> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "ligne incomplète",
            ));
        }
        let len = buf
            .iter()
            .position(|&b| b == b'\n')
            .map_or(buf.len(), |p| p + 1);
        if line.len() + len > MAX_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "ligne > 4 Mio"));
        }
        let finished = buf[len - 1] == b'\n';
        line.extend_from_slice(&buf[..len]);
        reader.consume(len);
        if finished {
            return serde_json::from_slice(&line)
                .map(Some)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
        }
    }
}

pub fn write_line<W: Write>(writer: &mut W, msg: &Outgoing<'_>) -> io::Result<()> {
    let encoded = serde_json::to_vec(msg)?;
    if encoded.len() + 1 > MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "ligne > 4 Mio"));
    }
    writer.write_all(&encoded)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn messages_roundtrip() {
        let input =
            br#"{"type":"notice.show","noticeId":"abc","message":"Salut","remainingSeconds":62}
{"type":"notice.hide","noticeId":"abc"}
{"type":"preview.start","maxWidth":1280,"fps":4}
{"type":"preview.stop"}
"#;
        let mut reader = io::BufReader::new(&input[..]);
        assert_eq!(
            read_line(&mut reader).unwrap(),
            Some(Incoming::NoticeShow {
                notice_id: "abc".into(),
                message: "Salut".into(),
                remaining_seconds: 62
            })
        );
        assert_eq!(
            read_line(&mut reader).unwrap(),
            Some(Incoming::NoticeHide {
                notice_id: "abc".into()
            })
        );
        assert_eq!(
            read_line(&mut reader).unwrap(),
            Some(Incoming::PreviewStart {
                max_width: 1280,
                fps: 4,
                show_indicator: true
            })
        );
        assert_eq!(
            serde_json::from_str::<Incoming>(
                r#"{"type":"preview.start","maxWidth":1280,"fps":4,"showIndicator":false}"#
            )
            .unwrap(),
            Incoming::PreviewStart {
                max_width: 1280,
                fps: 4,
                show_indicator: false
            }
        );
        assert_eq!(read_line(&mut reader).unwrap(), Some(Incoming::PreviewStop));
        assert_eq!(read_line(&mut reader).unwrap(), None);
        let mut out = Vec::new();
        write_line(&mut out, &Outgoing::NoticeDisplayed { notice_id: "abc" }).unwrap();
        assert_eq!(
            out,
            b"{\"type\":\"notice.displayed\",\"noticeId\":\"abc\"}\n"
        );
        let mut out = Vec::new();
        write_line(
            &mut out,
            &Outgoing::Hello {
                session_id: 1,
                pid: 2,
                version: "0.1",
            },
        )
        .unwrap();
        assert_eq!(
            out,
            b"{\"type\":\"hello\",\"sessionId\":1,\"pid\":2,\"version\":\"0.1\"}\n"
        );
        let mut out = Vec::new();
        write_line(
            &mut out,
            &Outgoing::PreviewFrame {
                jpeg_base64: "abc",
                width: 3,
                height: 2,
                captured_at_utc: "now",
            },
        )
        .unwrap();
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("\"jpegBase64\":\"abc\""));
        assert_eq!(
            serde_json::to_string(&Outgoing::NoticeFailed {
                notice_id: "n",
                reason: "x"
            })
            .unwrap(),
            "{\"type\":\"notice.failed\",\"noticeId\":\"n\",\"reason\":\"x\"}"
        );
        assert_eq!(
            serde_json::to_string(&Outgoing::PreviewFailed { reason: "x" }).unwrap(),
            "{\"type\":\"preview.failed\",\"reason\":\"x\"}"
        );
    }
    #[test]
    fn large_frames_and_recoverable_capture_error_keep_pipe_aligned() {
        let mut stream = Vec::new();
        let mut count = 0;
        for frame in [
            Ok(vec![b'a'; 150 * 1024]),
            Err("temporary GDI failure"),
            Ok(vec![b'b'; 150 * 1024]),
        ] {
            let Ok(bytes) = frame else { continue };
            count += 1;
            write_line(
                &mut stream,
                &Outgoing::PreviewFrame {
                    jpeg_base64: std::str::from_utf8(&bytes).unwrap(),
                    width: 800,
                    height: 600,
                    captured_at_utc: "2026-11-13T00:00:00Z",
                },
            )
            .unwrap();
        }
        assert_eq!(count, 2);
        let mut lines = io::BufReader::with_capacity(31, stream.as_slice());
        for expected in *b"ab" {
            let mut line = Vec::new();
            lines.read_until(b'\n', &mut line).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&line).unwrap();
            assert_eq!(value["jpegBase64"].as_str().unwrap().len(), 150 * 1024);
            assert_eq!(
                value["jpegBase64"].as_str().unwrap().as_bytes()[0],
                expected
            );
        }
        assert!(lines.fill_buf().unwrap().is_empty());
    }

    #[test]
    fn bounded_and_complete() {
        let bytes = vec![b'a'; MAX_LINE + 1];
        let mut over = io::BufReader::new(bytes.as_slice());
        assert_eq!(
            read_line(&mut over).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut incomplete = io::BufReader::new(&b"{}"[..]);
        assert_eq!(
            read_line(&mut incomplete).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
