// SPDX-License-Identifier: GPL-3.0-or-later
//! Print the configuration's JSON Schema.
fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&web888_config::Config::schema()).unwrap()
    );
}
