fn main() {
    println!("cargo:rerun-if-env-changed=MOAZ_ER_ACCESS_PUBLIC_KEY_BASE64URL");
    tauri_build::build()
}
