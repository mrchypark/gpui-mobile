use anyhow::{anyhow, Result};

pub(super) fn encode(username: &str, password: &[u8]) -> Result<Vec<u8>> {
    let length = u32::try_from(username.len())?;
    let mut payload = length.to_be_bytes().to_vec();
    payload.extend_from_slice(username.as_bytes());
    payload.extend_from_slice(password);
    Ok(payload)
}

pub(super) fn decode(payload: &[u8]) -> Result<(String, Vec<u8>)> {
    let length = u32::from_be_bytes(
        payload
            .get(..4)
            .ok_or_else(|| anyhow!("Truncated credential"))?
            .try_into()?,
    ) as usize;
    let end = 4usize
        .checked_add(length)
        .ok_or_else(|| anyhow!("Invalid account length"))?;
    let username = std::str::from_utf8(
        payload
            .get(4..end)
            .ok_or_else(|| anyhow!("Truncated account"))?,
    )?;
    Ok((username.to_owned(), payload[end..].to_vec()))
}
