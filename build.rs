fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // 请求 Windows 系统原生 UAC 管理员提权清单
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    }
}
