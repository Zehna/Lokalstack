#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    localstack_control_center_lib::run()
}
