// No console window when started at login on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use wasld::schedule::Timing;
#[cfg(not(feature = "miqyas"))]
use {
    std::sync::Arc,
    std::time::Duration,
    wasl_ipc::Paths,
    wasld::daemon::DaemonConfig,
    wasld::oauth::OAuthConfig,
    wasld::tokens::{Keychain, secrets_for},
};

fn main() {
    let mut home: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--version" | "-V" => {
                #[cfg(feature = "miqyas")]
                println!("wasld {} ({})", env!("CARGO_PKG_VERSION"), wasld::miqyas::BUILD);
                #[cfg(not(feature = "miqyas"))]
                println!("wasld {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--home" => match args.next() {
                // Portable or test installs keep everything in one folder.
                Some(dir) => home = Some(dir),
                None => fail("--home needs a folder"),
            },
            "--help" | "-h" => {
                println!("wasld: the Wasl background service. Usage: wasld [--home DIR] [--version]");
                return;
            }
            other => fail(&format!("unknown argument {other}")),
        }
    }
    #[cfg(feature = "miqyas")]
    let (paths, cfg) = {
        let paths = wasld::miqyas::home(home).unwrap_or_else(|e| fail(&e));
        if let Err(e) = paths.ensure() {
            fail(&e.to_string());
        }
        let cfg = wasld::miqyas::config(&paths).unwrap_or_else(|e| fail(&e));
        (paths, cfg)
    };
    #[cfg(not(feature = "miqyas"))]
    let (paths, cfg) = {
        let paths = match home.map(|h| Ok(Paths::at(h))).unwrap_or_else(Paths::for_user) {
            Ok(p) => p,
            Err(e) => fail(&e.to_string()),
        };
        if let Err(e) = paths.ensure() {
            fail(&e.to_string());
        }
        (paths.clone(), release_config(&paths))
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| fail(&e.to_string()));
    if let Err(e) = rt.block_on(wasld::serve(&paths, cfg, Timing::default())) {
        fail(&e);
    }
}

#[cfg(not(feature = "miqyas"))]
fn release_config(paths: &Paths) -> DaemonConfig {
    let device_name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Computer".into());
    // The keychain is shared by every daemon of this user: a separate home sees only its own
    // entries (#111), the default home keeps the names the installed app already has.
    let default_home = Paths::user_default().ok().map(|p| p.data);
    let stores = secrets_for(&paths.data, default_home.as_deref(), |service| {
        Arc::new(Keychain::new(service))
    });
    DaemonConfig {
        data_dir: paths.data.clone(),
        oauth: OAuthConfig::google(),
        secrets: stores.secrets,
        vault_keys: stores.vault_keys,
        drive_endpoints: None,
        open_browser: true,
        device_name,
        sign_in_timeout: Duration::from_secs(300),
        devices: Some(wasl_devices::DeviceConfig::default()),
        view_backend: None,
        view_refresh: wasld::views::REFRESH_EVERY,
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("wasld: {msg}");
    std::process::exit(1)
}
