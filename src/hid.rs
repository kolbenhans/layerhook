use std::time::{Duration, Instant};

const RAW_EPSIZE: usize = 32;
const READ_TIMEOUT: Duration = Duration::from_millis(1000);

pub type HidResult<T> = Result<T, String>;

fn transact(device: &hidapi::HidDevice, payload: &[u8]) -> HidResult<[u8; RAW_EPSIZE]> {
    let mut buf = [0u8; RAW_EPSIZE + 1];
    buf[1..1 + payload.len()].copy_from_slice(payload);
    device.write(&buf).map_err(|e| e.to_string())?;

    let deadline = Instant::now() + READ_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("timeout waiting for reply".to_string());
        }
        let mut resp = [0u8; RAW_EPSIZE];
        let n = device.read_timeout(&mut resp, remaining.as_millis() as i32).map_err(|e| e.to_string())?;
        if n > 0 && resp[0] == payload[0] && resp[1] == payload[1] {
            return Ok(resp);
        }
    }
}

pub fn set_layer(device: &hidapi::HidDevice, layer: u8) -> HidResult<()> {
    transact(device, &[0x02, 0xB0, layer])?;
    Ok(())
}

pub fn get_layer(device: &hidapi::HidDevice) -> HidResult<u8> {
    let resp = transact(device, &[0x02, 0xB1])?;
    Ok(resp[2])
}
