// Standalone diagnostic for the BCORNE raw HID layer interface (0xB0/0xB1) -
// not part of the layerhook GUI. Run manually after flashing new firmware to
// confirm the keyboard actually responds before wiring up the full app.
use layerhook::hid;
use qmk_via_api::scan::scan_keyboards;

fn main() {
    let devices = scan_keyboards().expect("HID scan failed");
    if devices.is_empty() {
        eprintln!("No VIA/Vial keyboard found.");
        std::process::exit(1);
    }
    for (i, d) in devices.iter().enumerate() {
        println!(
            "[{i}] {} ({:04X}:{:04X}) serial={:?}",
            d.product.clone().unwrap_or_default(),
            d.vendor_id,
            d.product_id,
            d.serial_number
        );
    }
    let dev = &devices[0];
    println!("-> using [0]");

    let api = hidapi::HidApi::new().expect("HidApi::new failed");
    let handle = api
        .device_list()
        .find(|d| d.usage_page() == dev.usage_page && d.vendor_id() == dev.vendor_id && d.product_id() == dev.product_id)
        .expect("device not in device_list")
        .open_device(&api)
        .expect("open_device failed");

    let before = hid::get_layer(&handle).expect("GET_LAYER failed");
    println!("Current layer: {before}");

    println!("SET_LAYER(1) ...");
    hid::set_layer(&handle, 1).expect("SET_LAYER(1) failed");
    let after_set = hid::get_layer(&handle).expect("GET_LAYER failed");
    println!("Layer after SET_LAYER(1): {after_set}");
    assert_eq!(after_set, 1, "Keyboard did not switch to layer 1");

    println!("Restoring starting layer ({before}) ...");
    hid::set_layer(&handle, before).expect("SET_LAYER(restore) failed");
    let restored = hid::get_layer(&handle).expect("GET_LAYER failed");
    println!("Layer after restore: {restored}");
    assert_eq!(restored, before, "Restore failed");

    println!("OK — SET_LAYER/GET_LAYER work.");
}
