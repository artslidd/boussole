use crate::{ClientMsg, MAX_FRAME_BYTES, MAX_SERVER_FRAME_BYTES, ProtocolError, ServerMsg};

/// Encode a server message as one JSON text frame.
pub fn encode(msg: &ServerMsg) -> String {
    // Cannot fail: every field is a string-keyed struct of plain values; the engine never
    // produces non-finite floats.
    serde_json::to_string(msg).expect("server messages always serialise")
}

/// Encode a client message as one JSON text frame.
pub fn encode_client(msg: &ClientMsg) -> String {
    serde_json::to_string(msg).expect("client messages always serialise")
}

/// Decode a client → server frame. Rejects frames over [`MAX_FRAME_BYTES`] (before parsing),
/// invalid JSON and unknown `type`s. Never panics.
pub fn decode_client(frame: &str) -> Result<ClientMsg, ProtocolError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            len: frame.len(),
            max: MAX_FRAME_BYTES,
        });
    }
    serde_json::from_str(frame).map_err(|e| ProtocolError::Malformed(e.to_string()))
}

/// Decode a server → client frame (capped at [`MAX_SERVER_FRAME_BYTES`]).
pub fn decode_server(frame: &str) -> Result<ServerMsg, ProtocolError> {
    if frame.len() > MAX_SERVER_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            len: frame.len(),
            max: MAX_SERVER_FRAME_BYTES,
        });
    }
    serde_json::from_str(frame).map_err(|e| ProtocolError::Malformed(e.to_string()))
}
