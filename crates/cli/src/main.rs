use anyhow::Result;
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use photoport_core::db::History;
use photoport_core::import::{history_path, run_import, ImportOptions, DCIM_ROOT};
use photoport_core::layout::{basename, DateStyle, Lang};
use photoport_core::live_photo::is_video;
use photoport_core::scan::{classify, scan_files};
use photoport_core::source::afc::{probe, AfcSource, DeviceStatus};
use photoport_core::{CancelFlag, PhotoSource, TransferEvent};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "photoport", version, about = "Transfert des photos d'un iPhone par USB")]
struct Cli {
    /// Parallel AFC connections.
    #[arg(short = 'j', long, default_value_t = photoport_core::source::afc::DEFAULT_CONCURRENCY, global = true)]
    jobs: usize,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the connected iPhone (name, model, iOS) and trust state.
    Info,
    /// Count photos and videos under /DCIM.
    Scan,
    /// Dry run: what the next import would copy into <dst>.
    Plan { dst: PathBuf },
    /// Copy only new files into <dst>, sorted by --layout. Ctrl-C cancels cleanly.
    Import {
        dst: PathBuf,
        /// Stop after N new files (Live Photos stay paired). 0 = all.
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: usize,
        /// Delete each verified file from the iPhone afterwards.
        #[arg(long)]
        delete_after: bool,
        /// Folder layout: month (AAAA/MM - Mois), day (AAAA/AAAA-MM-JJ), year (AAAA), flat.
        #[arg(long, default_value = "month", value_parser = ["month", "day", "year", "flat"])]
        layout: String,
    },
    /// Debug: list names in an AFC directory.
    #[command(hide = true)]
    Ls { path: String },
    /// Debug: copy a small AFC file to a local path.
    #[command(hide = true)]
    Get { path: String, out: PathBuf },
    /// Debug: save the iOS thumbnail of a DCIM file.
    #[command(hide = true)]
    Thumb { path: String, out: PathBuf },
    /// Debug: bytes [start, start+len) of an AFC file.
    #[command(hide = true)]
    Range { path: String, start: u64, len: usize, out: PathBuf },
    /// Debug: compare per-request reads with the single-handle block reader.
    #[command(hide = true)]
    BenchVideo { path: String, #[arg(default_value_t = 40)] mb: u64 },
    /// Debug: names and sizes in an AFC directory.
    #[command(hide = true)]
    Stat { path: String },

    /// Debug: thumbnails per second with -j connections.
    #[command(hide = true)]
    BenchThumbs {
        #[arg(default_value_t = 200)]
        n: usize,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_target(false)
        .init();
    let cli = Cli::parse();

    if let Cmd::Info = cli.cmd {
        return cmd_info().await;
    }
    let mut src = AfcSource::connect_first()
        .await
        .map_err(explain)?
        .with_concurrency(cli.jobs);
    match cli.cmd {
        Cmd::Info => unreachable!(),
        Cmd::Scan => cmd_scan(&mut src).await,
        Cmd::Plan { dst } => cmd_plan(&mut src, &dst).await,
        Cmd::Import { dst, limit, delete_after, layout } => {
            let style = DateStyle::from_key(&layout, Lang::Fr).expect("validated by clap");
            cmd_import(&mut src, &dst, limit, delete_after, style).await
        }
        Cmd::Ls { path } => {
            let names = src.list_names(&path).await?;
            println!("{} entrées", names.len());
            for n in &names {
                println!("  {n}");
            }
            Ok(())
        }
        Cmd::Range { path, start, len, out } => {
            let bytes = src.read_range(&path, start, len).await?;
            std::fs::write(&out, &bytes)?;
            println!("{} octets", bytes.len());
            Ok(())
        }
        Cmd::BenchVideo { path, mb } => {
            let t = Instant::now();
            for i in 0..mb / 4 {
                src.read_range(&path, i * 4 * 1_048_576, 4 * 1_048_576).await?;
            }
            let a = t.elapsed().as_secs_f64();
            struct Seq(u64, u64);
            #[async_trait::async_trait]
            impl photoport_core::source::afc::BlockPlan for Seq {
                async fn next(&mut self) -> Option<u64> {
                    self.0 += 1;
                    (self.0 <= self.1).then_some(self.0 - 1)
                }
                fn got(&mut self, _: u64, _: Vec<u8>) {}
            }
            let t = Instant::now();
            src.read_blocks(&path, 1_048_576, &mut Seq(mb, mb * 2)).await?;
            let b = t.elapsed().as_secs_f64();
            println!("ancien : {:.1} Mo/s   1 flux : {:.1} Mo/s", mb as f64 / a, mb as f64 / b);
            struct Strided { next: u64, step: u64, end: u64 }
            #[async_trait::async_trait]
            impl photoport_core::source::afc::BlockPlan for Strided {
                async fn next(&mut self) -> Option<u64> {
                    let i = self.next;
                    self.next += self.step;
                    (i < self.end).then_some(i)
                }
                fn got(&mut self, _: u64, _: Vec<u8>) {}
            }
            for n in [2u64, 4, 6] {
                let t = Instant::now();
                let mut tasks = Vec::new();
                for k in 0..n {
                    let p = path.clone();
                    tasks.push(tokio::spawn(async move {
                        let mut c = AfcSource::connect_first().await.unwrap();
                        let start = mb * 3 + k;
                        c.read_blocks(&p, 1_048_576, &mut Strided { next: start, step: n, end: mb * 4 }).await.unwrap();
                    }));
                }
                for t in tasks {
                    t.await?;
                }
                println!("{n} flux en parallèle : {:.1} Mo/s", mb as f64 / t.elapsed().as_secs_f64());
            }
            Ok(())
        }
        Cmd::Stat { path } => {
            for e in src.list(&path).await? {
                println!("{:>12}  {}", e.size, e.path);
            }
            Ok(())
        }
        Cmd::BenchThumbs { n } => bench_thumbs(&mut src, n, cli.jobs).await,
        Cmd::Thumb { path, out } => {
            let bytes = src.thumbnail(&path).await?;
            std::fs::write(&out, &bytes)?;
            println!("{} octets -> {}", bytes.len(), out.display());
            Ok(())
        }
        Cmd::Get { path, out } => {
            let bytes = src.read_small(&path).await?;
            std::fs::write(&out, &bytes)?;
            println!("{} octets -> {}", bytes.len(), out.display());
            Ok(())
        }
    }
}

async fn bench_thumbs(src: &mut AfcSource, n: usize, jobs: usize) -> Result<()> {
    let dir = "/DCIM/100APPLE";
    let mut names: Vec<String> = src
        .list_names(dir)
        .await?
        .into_iter()
        .filter(|n| !n.ends_with(".AAE"))
        .map(|n| format!("{dir}/{n}"))
        .collect();
    names.truncate(n);
    let total = names.len();
    let queue = std::sync::Arc::new(std::sync::Mutex::new(names));
    let t0 = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..jobs {
        let q = queue.clone();
        tasks.push(tokio::spawn(async move {
            let mut c = AfcSource::connect_first().await.expect("connect");
            let (mut ok, mut bytes) = (0usize, 0usize);
            loop {
                let next = q.lock().unwrap().pop();
                let Some(p) = next else { break };
                if let Ok(b) = c.thumbnail(&p).await {
                    ok += 1;
                    bytes += b.len();
                }
            }
            (ok, bytes)
        }));
    }
    let (mut ok, mut bytes) = (0, 0);
    for t in tasks {
        let (o, b) = t.await?;
        ok += o;
        bytes += b;
    }
    let secs = t0.elapsed().as_secs_f64();
    println!(
        "{ok}/{total} miniatures, {} en {secs:.2}s = {:.0} miniatures/s avec {jobs} connexions",
        human_bytes(bytes as u64),
        ok as f64 / secs
    );
    Ok(())
}

fn explain(e: photoport_core::Error) -> anyhow::Error {
    use photoport_core::Error::*;
    let hint = match &e {
        UsbmuxdUnavailable => match std::env::consts::OS {
            "windows" => "Installe l'app gratuite « Apple Devices » depuis le Microsoft Store, puis rebranche l'iPhone.",
            "linux" => "Installe usbmuxd : sudo apt install usbmuxd (ou dnf/pacman), puis sudo systemctl start usbmuxd.",
            _ => "Le service usbmuxd ne répond pas. Redémarre l'ordinateur.",
        },
        NoDevice => "Branche l'iPhone avec un câble USB (pas seulement en charge) et déverrouille-le.",
        NotPaired => "Déverrouille ton iPhone et appuie sur « Faire confiance », puis relance.",
        _ => "",
    };
    anyhow::anyhow!("{e}\n{hint}")
}

async fn cmd_info() -> Result<()> {
    match probe().await {
        DeviceStatus::Ready { device } => {
            println!("iPhone prêt");
            print_device(&device);
        }
        DeviceStatus::NotPaired { device } => {
            print_device(&device);
            return Err(explain(photoport_core::Error::NotPaired));
        }
        DeviceStatus::NoDevice => return Err(explain(photoport_core::Error::NoDevice)),
        DeviceStatus::UsbmuxdMissing => return Err(explain(photoport_core::Error::UsbmuxdUnavailable)),
    }
    Ok(())
}

fn print_device(d: &photoport_core::DeviceInfo) {
    println!("  Nom    : {}", d.name);
    println!("  Modèle : {}", d.model);
    println!("  iOS    : {}", d.ios_version);
    println!("  UDID   : {}", d.udid);
}

async fn cmd_scan(src: &mut AfcSource) -> Result<()> {
    let t0 = Instant::now();
    let files = src.walk(DCIM_ROOT).await?;
    let bytes: u64 = files.iter().map(|f| f.size).sum();
    let videos = files.iter().filter(|f| is_video(&f.path)).count();
    println!(
        "{} fichiers ({} photos, {} vidéos), {} — scan en {:.1}s",
        files.len(),
        files.len() - videos,
        videos,
        human_bytes(bytes),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

async fn cmd_plan(src: &mut AfcSource, dst: &Path) -> Result<()> {
    let t0 = Instant::now();
    let scan = scan_files(src, DCIM_ROOT).await?;
    std::fs::create_dir_all(dst)?;
    let db = History::open(&history_path(dst))?;
    let (plan, s) = classify(&scan.info, scan.files, &db)?;
    println!(
        "{} nouveaux ({}) · {} déjà transférés · {} sur l'iPhone — scan en {:.1}s",
        s.new_files,
        human_bytes(s.new_bytes),
        s.skipped_already_done,
        s.total_files,
        t0.elapsed().as_secs_f64()
    );
    for it in plan.iter().take(5) {
        println!("  {}  {}", human_bytes(it.entry.size), it.entry.path);
    }
    Ok(())
}

async fn cmd_import(
    src: &mut AfcSource,
    dst: &Path,
    limit: usize,
    delete_after: bool,
    style: DateStyle,
) -> Result<()> {
    let cancel = CancelFlag::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\nAnnulation… les fichiers déjà copiés sont conservés.");
            c.cancel();
        }
    });

    let bar = ProgressBar::new_spinner();
    bar.set_message("Analyse de l'iPhone…");
    bar.enable_steady_tick(std::time::Duration::from_millis(120));
    let b = bar.clone();
    let mut sink = move |ev: TransferEvent| match ev {
        TransferEvent::Scanned { new_files, new_bytes, skipped, .. } => {
            b.println(format!(
                "{new_files} nouveaux fichiers ({}), {skipped} déjà transférés",
                human_bytes(new_bytes)
            ));
        }
        TransferEvent::Start { total_bytes, .. } => {
            b.set_length(total_bytes);
            b.set_position(0);
            b.set_style(
                ProgressStyle::with_template(
                    "{bytes}/{total_bytes} [{bar:30}] {bytes_per_sec} reste {eta}  {msg}",
                )
                .unwrap()
                .progress_chars("##-"),
            );
        }
        TransferEvent::FileStart { path, .. } => b.set_message(basename(&path).to_string()),
        TransferEvent::FileProgress { bytes_delta } => b.inc(bytes_delta),
        TransferEvent::FileError { path, error } => b.println(format!("ERREUR {path}: {error}")),
        TransferEvent::FileDone { .. } => {}
    };
    let opts = ImportOptions { limit, delete_after, style, ..Default::default() };
    let r = run_import(src, dst, &opts, &cancel, &mut sink).await.map_err(explain)?;
    bar.finish_and_clear();

    let mbps = r.bytes as f64 / 1_048_576.0 / r.secs.max(1e-6);
    println!(
        "{} importés, {} en échec, {} en {:.1}s ({:.1} Mo/s){}",
        r.ok,
        r.failed,
        human_bytes(r.bytes),
        r.secs,
        mbps,
        if r.cancelled { " — annulé" } else { "" }
    );
    if delete_after {
        println!("{} fichiers supprimés de l'iPhone.", r.deleted);
    }
    if !r.errors.is_empty() {
        println!("Détails : {}", photoport_core::import::error_log_path(dst).display());
    }
    Ok(())
}

fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i + 1 < U.len() {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}
