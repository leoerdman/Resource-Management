// Без консольного окна на Windows в релизе.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    rmon_app_lib::run();
}
