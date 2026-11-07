//! The connection between the window manager and the graphics renderer process, in Rust: the
//! generated message types, the frame codec and the connection handshake.
//!
//! The types come from `dsa/protocol/renderer.proto` through `dsa-protocol-codegen`. The
//! handshake follows the window hosts': the renderer sends a `Hello` with its protocol version
//! and schema hash, and the window manager answers `HelloAccepted` or `HelloRejected` and
//! refuses a renderer generated from a different definition.

use std::fmt;
use std::time::Duration;

use dsa_transport::{Connection, FrameSender, TransportError};
use prost::Message;

/// The generated message types.
#[allow(clippy::all)]
pub mod types {
    include!("generated/dsa.renderer.v1.rs");
}

#[allow(clippy::all)]
mod schema {
    include!("generated/renderer_schema.rs");
}

pub use schema::{SCHEMA_HASH, SCHEMA_HASH_HEX};

use types::{envelope::Body, Envelope, Hello, HelloAccepted, HelloRejected, RejectReason, Role};

/// The protocol version of this build, from the definition's `ProtocolVersion` enum.
pub const PROTOCOL_VERSION: u32 = types::ProtocolVersion::Current as u32;

/// Encodes one envelope as one frame.
pub fn encode(envelope: &Envelope) -> Vec<u8> {
    envelope.encode_to_vec()
}

/// Encodes `body` as one frame.
pub fn encode_body(body: Body) -> Vec<u8> {
    encode(&Envelope { body: Some(body) })
}

/// Decodes one frame. Malformed input is an error, never a panic.
pub fn decode(frame: &[u8]) -> Result<Envelope, crate::DecodeError> {
    Envelope::decode(frame).map_err(|error| crate::DecodeError(error.to_string()))
}

/// Decodes one frame to its body. A frame without one is an error.
pub fn decode_body(frame: &[u8]) -> Result<Body, crate::DecodeError> {
    decode(frame)?
        .body
        .ok_or_else(|| crate::DecodeError("a renderer message without a body".into()))
}

/// Wraps `body` in an envelope and sends it as one frame.
pub fn send(connection: &mut dyn Connection, body: Body) -> Result<(), TransportError> {
    connection.send(&encode_body(body))
}

/// Wraps `body` in an envelope and sends it on a connection's sending half.
pub fn send_on(sender: &mut dyn FrameSender, body: Body) -> Result<(), TransportError> {
    sender.send(&encode_body(body))
}

/// This build's `Hello` for `role`.
pub fn local_hello(role: Role, build: &str) -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION,
        schema_hash: SCHEMA_HASH.to_vec(),
        role: role as i32,
        build: build.to_string(),
    }
}

/// Why a handshake did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    /// The connection failed or timed out.
    Transport(TransportError),
    /// The peer sent something that is not a usable handshake message.
    Malformed(String),
    /// The two sides do not match. On the window manager this is the refusal it sent; on
    /// the renderer it is the refusal it received.
    Rejected {
        reason: RejectReason,
        detail: String,
    },
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HandshakeError::Transport(error) => write!(f, "renderer connection: {error}"),
            HandshakeError::Malformed(why) => write!(f, "invalid renderer handshake: {why}"),
            HandshakeError::Rejected { detail, .. } => {
                write!(f, "the renderer connection was refused: {detail}")
            }
        }
    }
}

impl std::error::Error for HandshakeError {}

impl From<TransportError> for HandshakeError {
    fn from(error: TransportError) -> Self {
        HandshakeError::Transport(error)
    }
}

fn short(hash: &[u8]) -> String {
    crate::handshake_hex(hash).chars().take(12).collect()
}

/// Compares a renderer's `Hello` with the window manager's. Returns the refusal to send when
/// they differ.
pub fn check(peer: &Hello, local: &Hello) -> Result<(), (RejectReason, String)> {
    if peer.protocol_version != local.protocol_version {
        return Err((
            RejectReason::ProtocolVersionMismatch,
            format!(
                "the graphics renderer ({}) speaks version {}, and the window manager ({}) \
                 speaks version {}; install DSA again so both come from one release",
                peer.build, peer.protocol_version, local.build, local.protocol_version
            ),
        ));
    }
    if peer.schema_hash.len() != SCHEMA_HASH.len() {
        return Err((
            RejectReason::MalformedHello,
            format!(
                "the graphics renderer ({}) sent a schema hash of {} bytes; it must be {} bytes",
                peer.build,
                peer.schema_hash.len(),
                SCHEMA_HASH.len()
            ),
        ));
    }
    if peer.schema_hash != local.schema_hash {
        return Err((
            RejectReason::SchemaHashMismatch,
            format!(
                "the graphics renderer ({}) was built from a different definition (schema \
                 hash {}, expected {}); install DSA again so both come from one release",
                peer.build,
                short(&peer.schema_hash),
                short(&local.schema_hash)
            ),
        ));
    }
    if peer.role != Role::Renderer as i32 {
        return Err((
            RejectReason::UnexpectedRole,
            format!(
                "the peer ({}) did not say it is the graphics renderer (role {})",
                peer.build, peer.role
            ),
        ));
    }
    Ok(())
}

fn refuse(
    connection: &mut dyn Connection,
    local: &Hello,
    reason: RejectReason,
    detail: String,
) -> HandshakeError {
    let rejected = HelloRejected {
        reason: reason as i32,
        detail: detail.clone(),
        window_manager: Some(local.clone()),
    };
    // The refusal is a courtesy to the renderer; the connection closes either way.
    let _ = send(connection, Body::HelloRejected(rejected));
    connection.close();
    HandshakeError::Rejected { reason, detail }
}

/// The window manager's side: waits at most `timeout` for the renderer's `Hello`, checks it
/// and answers. Returns the renderer's `Hello` when accepted.
pub fn window_manager_accept(
    connection: &mut dyn Connection,
    local: &Hello,
    timeout: Duration,
) -> Result<Hello, HandshakeError> {
    let frame = connection.recv(Some(timeout))?;
    let peer = match decode(&frame) {
        Ok(Envelope {
            body: Some(Body::Hello(hello)),
        }) => hello,
        Ok(_) => {
            let detail = "the first message on the connection was not a Hello".to_string();
            return Err(refuse(
                connection,
                local,
                RejectReason::MalformedHello,
                detail,
            ));
        }
        Err(error) => {
            let detail = error.to_string();
            return Err(refuse(
                connection,
                local,
                RejectReason::MalformedHello,
                detail,
            ));
        }
    };
    if let Err((reason, detail)) = check(&peer, local) {
        return Err(refuse(connection, local, reason, detail));
    }
    send(
        connection,
        Body::HelloAccepted(HelloAccepted {
            window_manager: Some(local.clone()),
        }),
    )?;
    Ok(peer)
}

/// The renderer's side: sends `local` and waits at most `timeout` for the answer. Returns the
/// window manager's `Hello` when accepted.
pub fn renderer_connect(
    connection: &mut dyn Connection,
    local: &Hello,
    timeout: Duration,
) -> Result<Hello, HandshakeError> {
    send(connection, Body::Hello(local.clone()))?;
    let frame = connection.recv(Some(timeout))?;
    match decode(&frame) {
        Ok(Envelope {
            body: Some(Body::HelloAccepted(accepted)),
        }) => accepted.window_manager.ok_or_else(|| {
            HandshakeError::Malformed("HelloAccepted without the window manager's Hello".into())
        }),
        Ok(Envelope {
            body: Some(Body::HelloRejected(rejected)),
        }) => Err(HandshakeError::Rejected {
            reason: RejectReason::try_from(rejected.reason).unwrap_or(RejectReason::Unspecified),
            detail: rejected.detail,
        }),
        Ok(_) => Err(HandshakeError::Malformed(
            "the window manager answered with something other than HelloAccepted or \
             HelloRejected"
                .into(),
        )),
        Err(error) => Err(HandshakeError::Malformed(error.to_string())),
    }
}
