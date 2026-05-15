use std::error::Error;

use prost::Message;

pub(super) fn encode_grpc_message<M: Message>(message: &M) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut payload = Vec::with_capacity(message.encoded_len());
    message.encode(&mut payload)?;

    let mut body = Vec::with_capacity(5 + payload.len());
    body.push(0);
    body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    body.extend_from_slice(&payload);
    Ok(body)
}

pub(crate) fn decode_grpc_message<M: Message + Default>(body: &[u8]) -> Result<M, Box<dyn Error + Send + Sync>> {
    if body.len() < 5 {
        return Err("Sui gRPC response is missing message frame".into());
    }
    if body[0] != 0 {
        return Err("compressed Sui gRPC responses are not supported".into());
    }
    let len = u32::from_be_bytes(body[1..5].try_into()?) as usize;
    let end = 5usize.checked_add(len).ok_or("invalid Sui gRPC response frame length")?;
    if body.len() < end {
        return Err("truncated Sui gRPC response frame".into());
    }
    Ok(M::decode(&body[5..end])?)
}
