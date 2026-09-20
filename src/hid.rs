// Wire format matches qmk-via-api's raw HID transport (COMMAND_START=0x00,
// RAW_EPSIZE=32): a 33-byte write, report id 0x00 followed by 32 payload
// bytes. Family 0x02 / subcommands 0xB0 (SET_LAYER) and 0xB1 (GET_LAYER) are
// handled keymap-side in key_colors_hid.c (BCORNE), not standard VIA/Vial.
//
// The same raw HID interface is also used by the keypeek_layer_notify module
// (0xFF/0xF1-marked unsolicited packets, pushed whenever keypeek.AppImage is
// running and subscribed) and by Vial itself. transact() must skip any
// report that isn't the specific echo it's waiting for, or it'll misread
// someone else's packet as its own reply.
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
        // Unrelated packet (e.g. a keypeek layer-notify push) or a spurious
        // empty read on the timeout boundary — keep waiting for our echo.
    }
}

/// Sets the layer and waits for the keyboard's ack, so a successful return
/// means it actually applied — not just that the USB write went through.
pub fn set_layer(device: &hidapi::HidDevice, layer: u8) -> HidResult<()> {
    transact(device, &[0x02, 0xB0, layer])?;
    Ok(())
}

pub fn get_layer(device: &hidapi::HidDevice) -> HidResult<u8> {
    let resp = transact(device, &[0x02, 0xB1])?;
    Ok(resp[2])
}
