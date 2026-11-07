#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod descriptor;
mod dfu;
mod engine;
mod gui;
mod image;
mod platform;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use engine::{Engine, Event};

const USAGE: &str = "\
M-Track 2X2 firmware fix tool

  mtrack-fix                       start the GUI
  mtrack-fix check                 show device, descriptor and host/driver state
  mtrack-fix fix-host              remove the M-Audio driver / set up DFU access
  mtrack-fix flash [image] [--force]
                                   flash the built-in fixed firmware (or a given image)
  mtrack-fix revert                delete the upgrade image, back to factory firmware
  mtrack-fix backup <out.bin>      save the current upgrade image (0 bytes = factory only)
  mtrack-fix verify <image>        check an xflash image's CRCs
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        if let Err(e) = gui::run() {
            eprintln!("GUI failed: {e}");
            std::process::exit(2);
        }
        return;
    }
    attach_console();
    let code = match cli(&args) {
        Ok(msg) => {
            println!("\n{msg}");
            0
        }
        Err(e) => {
            eprintln!("\nERROR: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn cli(args: &[String]) -> anyhow::Result<String> {
    let sink: engine::Sink = Arc::new(|e: Event| match e {
        Event::Log(s) => println!("{s}"),
        Event::Step(_) | Event::Progress(_) | Event::Report(_) | Event::Done { .. } => {}
        Event::Attention(Some(s)) => println!(
            "\n{}\n  {}\n{}",
            "=".repeat(70),
            s.replace('\n', "\n  "),
            "=".repeat(70)
        ),
        Event::Attention(None) => {}
    });
    let engine = Engine::new(sink, Arc::new(AtomicBool::new(false)));
    let force = args.iter().any(|a| a == "--force");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    match positional.first().map(|s| s.as_str()) {
        Some("check") => {
            let r = engine.check();
            println!("Device (audio mode): {}", r.runtime.as_deref().unwrap_or("not found"));
            println!("Device (DFU mode):   {}", r.dfu_mode.as_deref().unwrap_or("not found"));
            if let Some(d) = r.descriptor {
                println!("Descriptor:          {}", d.describe());
            }
            if let Some(e) = &r.descriptor_error {
                println!("Descriptor:          could not read ({e})");
            }
            println!("Host ({}):", platform::OS_NAME);
            for i in &r.host.items {
                let mark = match i.ok {
                    Some(true) => "OK ",
                    Some(false) => "!! ",
                    None => " - ",
                };
                println!("  {mark}{}", i.text);
            }
            Ok(String::new())
        }
        Some("fix-host") => engine.fix_host(),
        // Internal: what the Linux GUI/CLI runs under pkexec. Not listed in the usage text.
        #[cfg(target_os = "linux")]
        Some("install-udev-rule") => platform::install_udev_rule(&|l| println!("{l}")).map(|()| String::new()),
        Some("flash") => {
            let fw = match positional.get(1) {
                Some(p) => {
                    let bytes = std::fs::read(p)?;
                    image::load(&bytes, p)?
                }
                None => image::builtin(),
            };
            engine.flash(&fw, force)
        }
        Some("revert") => engine.revert(),
        Some("backup") => {
            let out = positional
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("backup needs an output path"))?;
            engine.backup(&PathBuf::from(out))
        }
        Some("verify") => {
            let p = positional
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("verify needs an image path"))?;
            let fw = image::load(&std::fs::read(p)?, p)?;
            Ok(format!(
                "{p}: valid xflash image, version {}, {} bytes, CRCs ok, sha256 {}",
                fw.info.version, fw.info.length, fw.sha256_logical
            ))
        }
        _ => {
            print!("{USAGE}");
            Ok(String::new())
        }
    }
}

/// Windows GUI-subsystem binaries have no console; reuse the parent's (or create one) for CLI use.
fn attach_console() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Console::{
            AllocConsole, AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
        };
        // SAFETY: plain Win32 calls without preconditions.
        unsafe {
            // Redirected output (`mtrack-fix check > log`) already has a valid handle; attaching a
            // console would replace it. Only attach/allocate when there is no stdout at all.
            let out = GetStdHandle(STD_OUTPUT_HANDLE);
            if (out.is_null() || out == INVALID_HANDLE_VALUE) && AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                AllocConsole();
            }
        }
    }
}
