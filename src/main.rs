mod db;
mod timefmt;
mod tui;
use anyhow::Result;
use chrono::Local;
use clap::{Parser, Subcommand};
use db::{Db, StartOutcome};
use timefmt::{day_bounds, fmt_clock, fmt_short, now_ms, week_bounds};

#[derive(Parser)]
#[command(name = "enoch", about = "Track how long you have been doing activities")]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start tracking one or more activities
    Start {
        /// Activity names (created if new)
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Stop one or more activities; with no args, stop everything running
    Stop {
        names: Vec<String>,
    },
    /// Show what is running right now
    Now,
    /// List all activities with all-time totals
    List,
    /// Per-activity totals for today, this week, and all-time
    Report,
    /// Open the interactive panel (default when no subcommand is given)
    Tui,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("enoch: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let db = Db::open()?;
    match cli.command.unwrap_or(Cmd::Tui) {
        Cmd::Start { names } => cmd_start(&db, &names),
        Cmd::Stop { names } => cmd_stop(&db, &names),
        Cmd::Now => cmd_now(&db),
        Cmd::List => cmd_list(&db),
        Cmd::Report => cmd_report(&db),
        Cmd::Tui => tui::run(),
    }
}

fn cmd_start(db: &Db, names: &[String]) -> Result<()> {
    for name in names {
        match db.start(name)? {
            StartOutcome::Started => println!("started: {name}"),
            StartOutcome::AlreadyRunning => {
                let elapsed = db
                    .running()?
                    .into_iter()
                    .find(|r| r.name.eq_ignore_ascii_case(name))
                    .map(|r| fmt_clock(now_ms() - r.started_at));
                match elapsed {
                    Some(e) => println!("already running: {name} ({e})"),
                    None => println!("already running: {name}"),
                }
            }
        }
    }
    Ok(())
}

fn cmd_stop(db: &Db, names: &[String]) -> Result<()> {
    let stopped = db.stop(names)?;
    if stopped.is_empty() {
        println!("nothing to stop");
        return Ok(());
    }
    for s in &stopped {
        println!("stopped: {} ({})", s.name, fmt_short(s.ended_at - s.started_at));
    }
    Ok(())
}

fn cmd_now(db: &Db) -> Result<()> {
    let running = db.running()?;
    if running.is_empty() {
        println!("nothing running");
        return Ok(());
    }
    let now = now_ms();
    for r in &running {
        println!("{:>12}  {}", fmt_clock(now - r.started_at), r.name);
    }
    Ok(())
}

fn cmd_list(db: &Db) -> Result<()> {
    let rows = db.activities()?;
    if rows.is_empty() {
        println!("no activities yet (enoch start NAME)");
        return Ok(());
    }
    let now = now_ms();
    let w_name = rows.iter().map(|a| a.name.len()).max().unwrap_or(0).max("activity".len());
    let w_total = rows
        .iter()
        .map(|a| fmt_short(a.total).len())
        .max()
        .unwrap_or(0)
        .max("total".len());
    println!(
        "{:<w_name$}  {:>w_total$}  {:>8}  {}",
        "activity", "total", "sessions", "last used"
    );
    for a in &rows {
        let last = if a.last > 0 {
            format!("{} ago", fmt_short(now - a.last))
        } else {
            "-".into()
        };
        println!(
            "{:<w_name$}  {:>w_total$}  {:>8}  {}",
            a.name,
            fmt_short(a.total),
            a.sessions,
            last
        );
    }
    Ok(())
}

fn cmd_report(db: &Db) -> Result<()> {
    let n = Local::now().naive_local();
    let (df, dt) = day_bounds(n);
    let (wf, wt) = week_bounds(n);
    let all = db.totals(0, i64::MAX)?;
    if all.is_empty() {
        println!("no sessions yet (enoch start NAME)");
        return Ok(());
    }
    let day = db.totals(df, dt)?;
    let week = db.totals(wf, wt)?;

    let mut m: std::collections::BTreeMap<String, [i64; 3]> = std::collections::BTreeMap::new();
    for t in day {
        m.entry(t.name).or_insert([0; 3])[0] = t.ms;
    }
    for t in week {
        m.entry(t.name).or_insert([0; 3])[1] = t.ms;
    }
    for t in all {
        m.entry(t.name).or_insert([0; 3])[2] = t.ms;
    }
    let mut rows: Vec<(&String, &[i64; 3])> = m.iter().map(|(k, v)| (k, v)).collect();
    rows.sort_by_key(|(_, v)| std::cmp::Reverse(v[2]));

    let w0 = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0).max("activity".len());
    let w1 = rows.iter().map(|(_, v)| fmt_short(v[0]).len()).max().unwrap_or(0).max("today".len());
    let w2 = rows.iter().map(|(_, v)| fmt_short(v[1]).len()).max().unwrap_or(0).max("week".len());
    let w3 = rows
        .iter()
        .map(|(_, v)| fmt_short(v[2]).len())
        .max()
        .unwrap_or(0)
        .max("all-time".len());
    println!("{:<w0$}  {:>w1$}  {:>w2$}  {:>w3$}", "activity", "today", "week", "all-time");
    for (name, v) in rows {
        println!(
            "{:<w0$}  {:>w1$}  {:>w2$}  {:>w3$}",
            name,
            fmt_short(v[0]),
            fmt_short(v[1]),
            fmt_short(v[2])
        );
    }
    Ok(())
}
