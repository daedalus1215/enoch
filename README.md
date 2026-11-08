# enoch

A small, local time tracker. Start and stop activities from the shell, then
watch them live in a TUI or a desktop window — with month-by-month history, a 365-day heatmap,
and per-activity / per-category views. One binary, one SQLite file, no
server.

## Install

```sh
cargo build --release        # binary: target/release/enoch
# or: cargo install --path .

# CLI + TUI only, without the desktop window or tray icon:
cargo build --release --no-default-features
```

## Quick start

```sh
enoch start coding    # start tracking (creates "coding" if new)
enoch now             # what is running
enoch stop            # stop everything running
enoch                 # open the TUI (default when no subcommand is given)
```

## Commands

```
enoch start NAME...                                   start one or more activities
enoch stop [NAME...]                                  stop named activities, or everything
enoch now                                             show what is running
enoch list                                            all activities, grouped by category
enoch report                                          today / this week / all-time totals
enoch add NAME --from HH:MM [--to HH:MM] [--day D]    retroactive session
enoch export [--from D] [--to D]                      sessions as CSV to stdout
enoch cat new NAME | rename OLD NEW | move ACT CAT | rm NAME
enoch tui                                             open the interactive panel
enoch gui                                             open the desktop window
enoch tray                                            show a tray icon
```

- `D` is `YYYY-MM-DD`. `enoch add` defaults to today; `--to` defaults to
  `--from` + 3h.
- `enoch cat move ACT -` unassigns an activity.
- CSV columns: `activity,started,ended,duration_ms`. Timestamps are local
  ISO; an empty `ended` means the session is still running (duration is
  measured up to now).

## The TUI

Five modes, one key each — the status bar always shows the shortcuts:

- **1 · LIVE** — the home screen. Left rail: every activity, running
  ones marked with a dot. `j/k` selects, `enter` opens the selected
  activity's history, `x` stops it (if running). Right side: running
  timers on top, per-activity totals below; `t` cycles the period
  (today / this week / all-time).
- **2 · HISTORY** — a month grid; days with recorded time are marked.
  Pick a day to see its per-activity totals and individual sessions, and
  add, edit, or delete sessions from there. `y` opens the year view.
- **3 · MANAGE** — categories: create (`c`), rename (`r`), delete (`d`),
  and move an activity between categories (`m`; empty = unassigned).
- **4 · STATS** — 365-day heatmap of trailing activity with the selected
  day's detail below.
- **5 · ACTIVITY** — pick an activity from the left rail; the history
  panel scopes to it (month grid, sessions, year view). `tab` swaps focus
  between the rail and the history panel.

### Keys

```
LIVE (1)      s start · x stop · a add · t period · j/k rail · enter hist
HISTORY (2)   j/k select · ←/→ or n/p day · h/l month · t today
              a add session · e edit times · d delete
              y year view (j/k month · h/l year · enter open)
MANAGE (3)    c new cat · r rename · d del cat · m move
              j/k select (m needs an activity row)
STATS (4)     h/l day · j/k week · t today · enter history
ACTIVITY (5)  tab rail/history · j/k pick on rail · hist keys
COMMON        1/2/3/4/5 modes · ? help · q quit
PROMPTS       tab complete · enter save · esc cancel
```

`?` shows this screen inside the TUI; `esc` returns to live.

## The GUI

`enoch gui` opens a desktop window over the same database, with two tabs
(`Ctrl+1` / `Ctrl+2`).

**Live**

- **Start box** — type an activity and press Enter. Existing activities
  matching what you type appear as one-click buttons below it.
- **Activities rail** — every activity grouped by category; ▶ starts it,
  ■ stops it, running ones show a filled dot and their elapsed time.
- **Running** — a live clock per running activity, with Stop / Stop all.
- **Totals** — per-activity bars for today, this week, or all time.

**Day** — one lane per activity, each session a bar on a timeline:

- drag a bar's edge to change its start or end, or its middle to move it
- drag across empty lane space to record a new session
- right-click a bar to delete it or move it to another activity
- times snap to 5 minutes (hold `Shift` for 1)
- `Add lane` gives an activity with no sessions that day a lane to draw in
- `Left` / `Right` change day, `T` jumps to today, `Full day` shows 00:00–24:00
  instead of fitting the view to the day's sessions

Running sessions are drawn up to now but, as in the TUI, can't be edited
until stopped.

The window re-reads the database every second, so `enoch start` / `stop`
from a shell shows up without a restart. On Wayland its app id is
`enoch`, for window rules.

## The tray icon

`enoch tray` puts a small clock in your bar's system tray (any
StatusNotifierItem host: Waybar's `tray` module, the Omarchy shell, KDE…).
It is gray when nothing is running and takes the running activity's color
otherwise.

- **hover** — what is running and for how long
- **left click** — open the window (`enoch gui`)
- **menu** — stop each running activity (or all), start a recent one, open
  the window, or quit the tray

It reads the same database every second, so it stays in step with the CLI,
TUI and window. To start it with your session, add it to your compositor's
autostart, e.g. for Hyprland `exec-once = enoch tray` (or
`hl.exec_cmd("enoch tray")` in a Lua config).

## Data

- Stored in `~/.local/share/enoch/enoch.db`, or
  `$XDG_DATA_HOME/enoch/enoch.db` if `XDG_DATA_HOME` is set. It is a
  single SQLite file — back it up or copy it between machines like any
  file.
- Tables: `activities` (name + optional category), `sessions`
  (`ended_at IS NULL` = still running), `categories`.
- Times are unix milliseconds; day/week boundaries are computed in local
  time.
