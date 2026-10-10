# enoch

A small, local time tracker. Start and stop activities from the shell, then
watch them live in a TUI — with month-by-month history, a 365-day heatmap,
and per-activity / per-category views. One binary, one SQLite file, no
server.

## Install

```sh
cargo build --release        # binary: target/release/enoch
# or: cargo install --path .
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

## Data

- Stored in `~/.local/share/enoch/enoch.db`, or
  `$XDG_DATA_HOME/enoch/enoch.db` if `XDG_DATA_HOME` is set. It is a
  single SQLite file — back it up or copy it between machines like any
  file.
- Tables: `activities` (name + optional category), `sessions`
  (`ended_at IS NULL` = still running), `categories`.
- Times are unix milliseconds; day/week boundaries are computed in local
  time.
