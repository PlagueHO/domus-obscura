//! Typed wire events. Connection identity and workflow authority are deliberately
//! absent: the transport and application attach those independently.
use super::protocol::{Fields, protocol};
use crate::Result;
use std::net::{Ipv4Addr, SocketAddrV4};

#[derive(Debug)]
pub(super) enum ServerEvent {
    Address {
        username: String,
        address: SocketAddrV4,
    },
    Connect {
        username: String,
        kind: String,
        address: SocketAddrV4,
        token: u32,
    },
    CannotConnect(u32),
}

#[derive(Debug)]
pub(super) enum PeerEvent {
    Offer {
        token: u32,
        path: String,
        bytes: u64,
    },
    Position {
        path: String,
        place: u32,
    },
    Denied {
        path: String,
    },
    Failed {
        path: String,
    },
    Search(Vec<u8>),
    Folder(Vec<u8>),
}

#[derive(Debug)]
pub(super) enum Event {
    Server(ServerEvent),
    Peer(PeerEvent),
}

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Server,
    Peer,
}

fn address(fields: &mut Fields<'_>) -> Result<SocketAddrV4> {
    let ip = Ipv4Addr::from(fields.u32()?);
    let port = u16::try_from(fields.u32()?).map_err(|_| protocol())?;
    Ok(SocketAddrV4::new(ip, port))
}

fn count(fields: &mut Fields<'_>, maximum: u32) -> Result<u32> {
    let count = fields.u32()?;
    if count > maximum {
        return Err(protocol());
    }
    Ok(count)
}

fn text_list(fields: &mut Fields<'_>, maximum: u32, length: usize) -> Result<u32> {
    let count = count(fields, maximum)?;
    for _ in 0..count {
        fields.text(length)?;
    }
    Ok(count)
}

fn notification(code: u32, fields: &mut Fields<'_>) -> Result<bool> {
    match code {
        32 | 130 => {}
        104 | 83 | 84 => {
            fields.u32()?;
        }
        7 => {
            fields.text(30)?;
            if fields.u32()? > 2 {
                return Err(protocol());
            }
            fields.boolean()?;
        }
        36 => {
            fields.text(30)?;
            for _ in 0..5 {
                fields.u32()?;
            }
        }
        64 => {
            for _ in 0..3 {
                let rooms = text_list(fields, 10_000, 128)?;
                if count(fields, 10_000)? != rooms {
                    return Err(protocol());
                }
                for _ in 0..rooms {
                    fields.u32()?;
                }
            }
            text_list(fields, 10_000, 128)?;
        }
        69 => {
            text_list(fields, 25_000, 30)?;
        }
        102 => {
            for _ in 0..count(fields, 10)? {
                fields.text(30)?;
                fields.u32()?;
                fields.u32()?;
            }
        }
        160 => {
            text_list(fields, 10_000, 4096)?;
        }
        93 => {
            if fields.take(1)? != [3] || fields.u32()? != 49 {
                return Err(protocol());
            }
            fields.text(30)?;
            fields.u32()?;
            fields.text(4096)?;
        }
        26 => {
            fields.text(30)?;
            fields.u32()?;
            fields.text(4096)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Known unsolicited server notifications are discarded only here, under the
/// transport's rate bound. Unknown messages fail; operation readers never skip.
pub(super) fn decode(kind: Kind, code: u32, body: Vec<u8>) -> Result<Option<Event>> {
    let mut fields = Fields(&body);
    let event = match kind {
        Kind::Server => {
            let event = match code {
                3 => {
                    let username = fields.text(30)?.to_owned();
                    super::username(&username).map_err(|_| protocol())?;
                    let address = address(&mut fields)?;
                    if !fields.0.is_empty() {
                        fields.u32()?;
                        fields.take(2)?;
                    }
                    ServerEvent::Address { username, address }
                }
                18 => {
                    let username = fields.text(30)?.to_owned();
                    super::username(&username).map_err(|_| protocol())?;
                    let kind = fields.text(1)?.to_owned();
                    if !matches!(kind.as_str(), "P" | "F" | "D") {
                        return Err(protocol());
                    }
                    let address = address(&mut fields)?;
                    let token = fields.u32()?;
                    fields.boolean()?;
                    if !fields.0.is_empty() {
                        fields.u32()?;
                        fields.u32()?;
                    }
                    ServerEvent::Connect {
                        username,
                        kind,
                        address,
                        token,
                    }
                }
                1001 => ServerEvent::CannotConnect(fields.u32()?),
                41 => {
                    return Err(crate::Error::Remote(
                        "Soulseek account signed in elsewhere; session closed",
                    ));
                }
                _ => {
                    if !notification(code, &mut fields)? {
                        return Err(protocol());
                    }
                    fields.end()?;
                    return Ok(None);
                }
            };
            Event::Server(event)
        }
        Kind::Peer => {
            let event = match code {
                40 => {
                    if fields.u32()? != 1 {
                        return Err(protocol());
                    }
                    PeerEvent::Offer {
                        token: fields.u32()?,
                        path: fields.text(4096)?.to_owned(),
                        bytes: fields.u64()?,
                    }
                }
                44 => PeerEvent::Position {
                    path: fields.text(4096)?.to_owned(),
                    place: fields.u32()?,
                },
                46 => PeerEvent::Failed {
                    path: fields.text(4096)?.to_owned(),
                },
                50 => {
                    let path = fields.text(4096)?.to_owned();
                    fields.text(4096)?;
                    PeerEvent::Denied { path }
                }
                9 => return Ok(Some(Event::Peer(PeerEvent::Search(body)))),
                37 => return Ok(Some(Event::Peer(PeerEvent::Folder(body)))),
                _ => return Err(protocol()),
            };
            Event::Peer(event)
        }
    };
    fields.end()?;
    Ok(Some(event))
}
