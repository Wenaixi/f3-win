fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // 请求 Windows 系统原生 UAC 管理员提权清单
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");

        // 嵌入原生 Windows 应用程序图标与版本元数据
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app_icon.ico");
        res.set("ProductName", "F3-Win");
        res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
        res.set("FileVersion", env!("CARGO_PKG_VERSION"));
        res.set("FileDescription", "F3-Win - Flash Memory Fraud Verification Tool");
        res.set("LegalCopyright", "Copyright (C) 2026 F3-Win Contributors");
        let _ = res.compile();
    }
}
