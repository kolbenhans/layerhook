use layerhook::hid;
use qmk_via_api::scan::scan_keyboards;
use std::thread::sleep;
use std::time::Duration;

fn main() {
    let devices = scan_keyboards().expect("HID scan failed");
    let dev = devices.iter().find(|d| d.product.as_deref() == Some("BCORNE")).expect("BCORNE not found");

    let api = hidapi::HidApi::new().expect("HidApi::new failed");
    let handle = api
        .device_list()
        .find(|d| d.usage_page() == dev.usage_page && d.vendor_id() == dev.vendor_id && d.product_id() == dev.product_id)
        .expect("device not in device_list")
        .open_device(&api)
        .expect("open_device failed");

    let before = hid::get_layer(&handle).expect("GET_LAYER failed");
    println!("Starting layer: {before}");

    for layer in 1..=4u8 {
        println!("-> Layer {layer} (3s)");
        hid::set_layer(&handle, layer).expect("SET_LAYER failed");
        sleep(Duration::from_secs(3));
    }

    println!("-> back to layer {before}");
    hid::set_layer(&handle, before).expect("SET_LAYER (restore) failed");
}
