use futures_util::future::Either;
use futures_util::stream::StreamExt;
use std::collections::{BTreeMap, HashMap};
use std::io::{self};
use structopt::StructOpt;
use termion::raw::IntoRawMode;
use tokio::prelude::*;
use tui::backend::Backend;
use tui::backend::TermionBackend;
use tui::layout::{Constraint, Direction, Layout};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{Block, Borders, Paragraph, Text, Widget};
use tui::Terminal;

const DRAW_EVERY: std::time::Duration = std::time::Duration::from_millis(200);
const WINDOW: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, StructOpt)]
/// A live profile visualizer.
///
/// Pipe the output of the appropriate `bpftrace` command into this program, and enjoy.
/// Happy profiling!
struct Opt {
    /// Treat input as a replay of a trace and emulate time accordingly.
    #[structopt(long)]
    replay: bool,
}

#[derive(Debug, Default)]
struct Thread {
    window: BTreeMap<usize, String>,
}

fn main() -> Result<(), io::Error> {
    let opt = Opt::from_args();

    if termion::is_tty(&io::stdin().lock()) {
        eprintln!("Don't type input to this program, that's silly.");
        return Ok(());
    }

    let stdout = io::stdout().into_raw_mode()?;
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut tids = BTreeMap::new();
    let mut inframe = None;
    let mut stack = String::new();

    terminal.hide_cursor()?;
    terminal.clear()?;
    terminal.draw(|mut f| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(2)
            .constraints([Constraint::Percentage(100)].as_ref())
            .split(f.size());
        Block::default()
            .borders(Borders::ALL)
            .title("Common thread fan-out points")
            .title_style(Style::default().fg(Color::Magenta).modifier(Modifier::BOLD))
            .render(&mut f, chunks[0]);
    })?;

    // a _super_ hacky way for us to get input from the TTY
    let tty = termion::get_tty()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        use termion::input::TermRead;
        for key in tty.keys() {
            if let Err(_) = tx.send(key) {
                return;
            }
        }
    });

    let mut rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let lines = stdin.lines().map(Either::Left);
        let rx = rx.map(Either::Right);
        let mut input = futures_util::stream::select(lines, rx);

        let mut lastprint = 0;
        let mut lasttime = 0;
        while let Some(got) = input.next().await {
            match got {
                Either::Left(line) => {
                    let line = line.unwrap();
                    if line.starts_with("Error") || line.starts_with("Attaching") {
                    } else if !line.starts_with(' ') || line.is_empty() {
                        if let Some((time, tid)) = inframe {
                            // new frame starts, so finish the old one
                            // skip empty stack frames
                            if !stack.is_empty() {
                                let nxt_stack = String::with_capacity(stack.capacity());
                                let mut stack = std::mem::replace(&mut stack, nxt_stack);

                                // remove trailing ;
                                let stackn = stack.len();
                                stack.truncate(stackn - 1);

                                tids.entry(tid)
                                    .or_insert_with(Thread::default)
                                    .window
                                    .insert(time, stack);

                                if opt.replay && lasttime != 0 && time - lasttime > 1_000_000 {
                                    tokio::time::delay_for(std::time::Duration::from_nanos(
                                        (time - lasttime) as u64,
                                    ))
                                    .await;
                                }
                                lasttime = time;
                                if std::time::Duration::from_nanos((time - lastprint) as u64)
                                    > DRAW_EVERY
                                {
                                    draw(&mut terminal, &mut tids)?;
                                    lastprint = time;
                                }
                            }
                            inframe = None;
                        }

                        if !line.is_empty() {
                            // read time + tid
                            let mut fields = line.split_whitespace();
                            let time = fields
                                .next()
                                .expect("no time given for frame")
                                .parse::<usize>()
                                .expect("invalid time");
                            let tid = fields
                                .next()
                                .expect("no tid given for frame")
                                .parse::<usize>()
                                .expect("invalid tid");
                            inframe = Some((time, tid));
                        }
                    } else {
                        assert!(inframe.is_some());
                        stack.push_str(line.trim());
                        stack.push(';');
                    }
                }
                Either::Right(key) => {
                    let key = key?;
                    if let termion::event::Key::Char('q') = key {
                        break;
                    }
                }
            }
        }

        terminal.clear()?;
        Ok(())
    })
}

fn draw<B: Backend>(
    terminal: &mut Terminal<B>,
    threads: &mut BTreeMap<usize, Thread>,
) -> Result<(), io::Error> {
    // keep our window relatively short
    let mut latest = 0;
    for thread in threads.values() {
        if let Some(&last) = thread.window.keys().next_back() {
            latest = std::cmp::max(latest, last);
        }
    }
    if latest > WINDOW.as_nanos() as usize {
        for thread in threads.values_mut() {
            // trim to last 5 seconds
            thread.window = thread
                .window
                .split_off(&(latest - WINDOW.as_nanos() as usize));
        }
    }

    // now only reading
    let threads = &*threads;

    let mut lines = Vec::new();
    let mut hits = HashMap::new();
    let mut maxes = BTreeMap::new();
    for (_, thread) in threads {
        // add up across the window
        let mut max: Option<(&str, usize)> = None;
        for (&time, stack) in &thread.window {
            latest = std::cmp::max(latest, time);
            let mut at = stack.len();
            while let Some(stack_start) = stack[..at].rfind(';') {
                at = stack_start;
                let stack = &stack[at + 1..];
                let count = hits.entry(stack).or_insert(0);
                *count += 1;
                if let Some((_, max_count)) = max {
                    if *count >= max_count {
                        max = Some((stack, *count));
                    }
                } else {
                    max = Some((stack, *count));
                }
            }
        }

        if let Some((stack, count)) = max {
            let e = maxes.entry(stack).or_insert((0, 0));
            e.0 += 1;
            e.1 += count;
        }
        hits.clear();
    }

    if maxes.is_empty() {
        return Ok(());
    }

    let max = *maxes.values().map(|(_, count)| count).max().unwrap() as f64;

    // sort by where most threads are
    let mut maxes: Vec<_> = maxes.into_iter().collect();
    maxes.sort_by_key(|(_, (nthreads, _))| *nthreads);

    for (stack, (nthreads, count)) in maxes.iter().rev() {
        let count = *count;
        let nthreads = *nthreads;

        if stack.find(';').is_none() {
            // this thread just shares the root frame
            continue;
        }

        if count == 1 {
            // this thread only has one sample ever, let's reduce noise...
            continue;
        }

        let red = (128.0 * count as f64 / max) as u8;
        let color = Color::Rgb(255, 128 - red, 128 - red);

        if nthreads == 1 {
            lines.push(Text::styled(
                format!("A thread fanned out from here {} times\n", count),
                Style::default().modifier(Modifier::BOLD).fg(color),
            ));
        } else {
            lines.push(Text::styled(
                format!(
                    "{} threads fanned out from here {} times\n",
                    nthreads, count
                ),
                Style::default().modifier(Modifier::BOLD).fg(color),
            ));
        }

        for (i, frame) in stack.split(';').enumerate() {
            // https://github.com/alexcrichton/rustc-demangle/issues/34
            if i == 0 {
                lines.push(Text::styled(
                    format!("  {}\n", rustc_demangle::demangle(frame)),
                    Style::default(),
                ));
            } else {
                lines.push(Text::styled(
                    format!("  {}\n", rustc_demangle::demangle(frame)),
                    Style::default().modifier(Modifier::DIM),
                ));
            }
        }
        lines.push(Text::raw("\n"));
    }

    terminal.draw(|mut f| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(2)
            .constraints([Constraint::Percentage(100)].as_ref())
            .split(f.size());

        Paragraph::new(lines.iter())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Common thread fan-out points")
                    .title_style(Style::default().fg(Color::Magenta).modifier(Modifier::BOLD)),
            )
            .render(&mut f, chunks[0]);
    })?;

    Ok(())
}
