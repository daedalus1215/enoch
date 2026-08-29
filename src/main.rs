mod db;
mod theme;
mod timefmt;
mod tui;
use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};
use db::{Db, Scope, StartOutcome};
use timefmt::{
    day_bounds, fmt_clock, fmt_range, fmt_short, iso_local, local_midnight_ms, now_ms, parse_hm_on,
    week_bounds,
};

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
    /// Add a retroactive session (closed, explicit times)
    Add {
        /// Activity name (created if new)
        name: String,
        /// Day as YYYY-MM-DD (default today)
        #[arg(long)]
        day: Option<String>,
        /// Start time HH:MM (default 09:00)
        #[arg(long, default_value = "09:00")]
        from: String,
        /// End time HH:MM (default from + 1h)
        #[arg(long)]
        to: Option<String>,
    },
    /// Export sessions as CSV to stdout
    Export {
        /// Only sessions starting on/after this day (YYYY-MM-DD)
        #[arg(long)]
        from: Option<String>,
        /// Only sessions starting on/before this day (YYYY-MM-DD)
        #[arg(long)]
        to: Option<String>,
    },
    /// Manage activity categories (new, rename, move, rm)
    Cat {
        #[command(subcommand)]
        action: CatCmd,
    },
}

#[derive(Subcommand)]
enum CatCmd {
    /// Create a category
    New {
        name: String,
    },
    /// Rename a category
    Rename {
        old: String,
        new: String,
    },
    /// Move an activity into a category; "-" unassigns
    Move {
        activity: String,
        category: String,
    },
    /// Delete a category (its activities become unassigned)
    Rm {
        name: String,
    },
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
        Cmd::Add { name, day, from, to } => cmd_add(&db, &name, &day, &from, &to),
        Cmd::Export { from, to } => cmd_export(&db, &from, &to),
        Cmd::Cat { action } => cmd_cat(&db, action),
        Cmd::Tui => tui::run(),
    }
}

fn cmd_start(db: &Db, names: &[String]) -> Result<()> {
    for name in names {
        match db.start(name)? {
            StartOutcome::Started => println!("started: {name}"),
            StartOutcome::StartedNew => println!("started: {name} (new activity)"),
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
    let groups = group_by_category(db, &rows)?;
    let w_name = rows.iter().map(|a| a.name.len()).max().unwrap_or(0).max("activity".len());
    let w_total = rows
        .iter()
        .map(|a| fmt_short(a.total).len())
        .max()
        .unwrap_or(0)
        .max("total".len());
    for (cat, members) in groups {
        let sub: i64 = members.iter().map(|a| a.total).sum();
        println!("{} ({})", cat, fmt_short(sub));
        for a in members {
            let last = if a.last > 0 {
                format!("{} ago", fmt_short(now - a.last))
            } else {
                "-".into()
            };
            println!(
                "  {:<w_name$}  {:>w_total$}  {:>8}  {}",
                a.name,
                fmt_short(a.total),
                a.sessions,
                last
            );
        }
    }
    Ok(())
}

/// Group activity rows by category label; unassigned first, then by name.
fn group_by_category(
    db: &Db,
    rows: &[db::Activity],
) -> Result<Vec<(String, Vec<db::Activity>)>> {
    let map: std::collections::HashMap<String, Option<String>> = db
        .category_map()?
        .into_iter()
        .map(|(n, c)| (n.to_lowercase(), c))
        .collect();
    let mut groups: std::collections::BTreeMap<Option<String>, Vec<db::Activity>> =
        std::collections::BTreeMap::new();
    for a in rows {
        let cat = map
            .get(&a.name.to_lowercase())
            .cloned()
            .flatten();
        groups.entry(cat).or_default().push(a.clone());
    }
    let mut out: Vec<(String, Vec<db::Activity>)> = Vec::new();
    for (cat, members) in groups {
        let label = match cat {
            None => "no category".to_string(),
            Some(c) => c,
        };
        out.push((label, members));
    }
    out.sort_by(|a, b| {
        (a.0 == "no category")
            .cmp(&(b.0 == "no category"))
            .then_with(|| a.0.cmp(&b.0))
    });
    Ok(out)
}

fn cmd_report(db: &Db) -> Result<()> {
    let n = Local::now().naive_local();
    let (df, dt) = day_bounds(n.date());
    let (wf, wt) = week_bounds(n.date());
    let all = db.totals(0, i64::MAX, &Scope::All)?;
    if all.is_empty() {
        println!("no sessions yet (enoch start NAME)");
        return Ok(());
    }
    let day = db.totals(df, dt, &Scope::All)?;
    let week = db.totals(wf, wt, &Scope::All)?;

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
    let mut rows: Vec<(String, [i64; 3])> = m.into_iter().collect();
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

    let map: std::collections::HashMap<String, Option<String>> = db
        .category_map()?
        .into_iter()
        .map(|(n, c)| (n.to_lowercase(), c))
        .collect();
    let mut groups: std::collections::BTreeMap<Option<String>, Vec<(String, [i64; 3])>> =
        std::collections::BTreeMap::new();
    for (name, v) in rows {
        let cat = map.get(&name.to_lowercase()).cloned().flatten();
        groups.entry(cat).or_default().push((name, v));
    }
    println!("{:<w0$}  {:>w1$}  {:>w2$}  {:>w3$}", "activity", "today", "week", "all-time");
    let mut keys: Vec<&Option<String>> = groups.keys().collect();
    keys.sort_by(|a, b| {
        (a.is_none())
            .cmp(&b.is_none())
            .then_with(|| a.as_deref().unwrap_or_default().cmp(b.as_deref().unwrap_or_default()))
    });
    for key in keys {
        let members = &groups[key];
        let label: String = match key {
            None => "no category".to_string(),
            Some(c) => c.clone(),
        };
        println!("{label}");
        let mut sub = [0i64; 3];
        for (name, v) in members {
            for (i, x) in v.iter().enumerate() {
                sub[i] += x;
            }
            println!(
                "  {:<w0$}  {:>w1$}  {:>w2$}  {:>w3$}",
                name,
                fmt_short(v[0]),
                fmt_short(v[1]),
                fmt_short(v[2])
            );
        }
        println!(
            "  {:<w0$}  {:>w1$}  {:>w2$}  {:>w3$}",
            "total",
            fmt_short(sub[0]),
            fmt_short(sub[1]),
            fmt_short(sub[2])
        );
    }
    Ok(())
}

fn cmd_add(db: &Db, name: &str, day: &Option<String>, from: &str, to: &Option<String>) -> Result<()> {
    let d = match day {
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .with_context(|| format!("bad --day {s}, want YYYY-MM-DD"))?,
        None => Local::now().date_naive(),
    };
    let s = parse_hm_on(d, from)
        .with_context(|| format!("bad --from {from}, want HH:MM"))?;
    let e = match to {
        Some(t) => parse_hm_on(d, t).with_context(|| format!("bad --to {t}, want HH:MM"))?,
        None => s + 3_600_000,
    };
    if e <= s {
        anyhow::bail!("end must be after start");
    }
    let created = db.add_session(name, s, e)?;
    let note = if created { " · new activity" } else { "" };
    println!("added: {name} ({}) — {}{note}", fmt_range(s, e), fmt_short(e - s));
    Ok(())
}

fn cmd_cat(db: &Db, action: CatCmd) -> Result<()> {
    match action {
        CatCmd::New { name } => {
            db.create_category(&name)?;
            println!("created: {name}");
        }
        CatCmd::Rename { old, new } => {
            let id = db
                .category_id(&old)?
                .ok_or_else(|| anyhow::anyhow!("no such category: {old}"))?;
            db.rename_category(id, &new)?;
            println!("renamed: {old} -> {new}");
        }
        CatCmd::Move { activity, category } => {
            let cat = if category.is_empty() || category == "-" {
                None
            } else {
                Some(category.as_str())
            };
            db.set_activity_category(&activity, cat)?;
            match cat {
                Some(c) => println!("moved: {activity} -> {c}"),
                None => println!("unassigned: {activity}"),
            }
        }
        CatCmd::Rm { name } => {
            let id = db
                .category_id(&name)?
                .ok_or_else(|| anyhow::anyhow!("no such category: {name}"))?;
            db.delete_category(id)?;
            println!("deleted: {name}");
        }
    }
    Ok(())
}

fn cmd_export(db: &Db, from: &Option<String>, to: &Option<String>) -> Result<()> {
    let f = match from {
        Some(s) => day_start_ms(s, "from")?,
        None => 0,
    };
    let t = match to {
        Some(s) => {
            let d = parse_day(s, "to")?;
            local_midnight_ms(d + chrono::Duration::days(1))
        }
        None => i64::MAX,
    };
    let rows = db.sessions_in(f, t, &Scope::All)?;
    println!("activity,started,ended,duration_ms");
    let now = now_ms();
    for r in &rows {
        let (end, dur) = match r.ended_at {
            Some(e) => (iso_local(e), e - r.started_at),
            None => (String::new(), now - r.started_at),
        };
        println!("{},{},{},{dur}", csv_field(&r.name), iso_local(r.started_at), end);
    }
    Ok(())
}

/// Parse `YYYY-MM-DD`; the context label goes into the error message.
fn parse_day(s: &str, what: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("bad --{what} {s}, want YYYY-MM-DD"))
}

fn day_start_ms(s: &str, what: &str) -> Result<i64> {
    Ok(local_midnight_ms(parse_day(s, what)?))
}

/// RFC 4180 quoting for a CSV field.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
