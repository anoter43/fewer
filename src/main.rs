use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, ClearType};
use crossterm::{cursor, queue};
use std::env;
use std::fs;
use std::io::{self, Write};

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return Ok(());
    }
    if args.is_empty() {
        eprintln!("fewer: no file specified");
        print_usage();
        std::process::exit(1);
    }

    let contents = fs::read_to_string(&args[0])?;
    let lines: Vec<&str> = contents.lines().collect();

    terminal::enable_raw_mode()?;
    let result = pager(&lines);
    terminal::disable_raw_mode()?;
    result
}

fn print_usage() {
    println!("Usage: fewer [FILE]");
    println!();
    println!("A minimal pager.");
    println!();
    println!("Keys:");
    println!("  j/Arrow Down   scroll down one line");
    println!("  k/Arrow Up     scroll up one line");
    println!("  Space/PgDn     scroll down one screen");
    println!("  b/PgUp         scroll up one screen");
    println!("  g/Home         go to the first line");
    println!("  G/End          go to the last line");
    println!("  /              search forward (n/N for next/previous)");
    println!("  q              quit");
    println!();
    println!("Options:");
    println!("  -h, --help     show this help message");
}

fn pager(lines: &[&str]) -> io::Result<()> {
    let mut stdout = io::stdout();
    let mut offset = 0usize; // index of the first visible line
    let mut query: Option<String> = None; // active search term
    let mut match_line: Option<usize> = None; // line the last search landed on

    loop {
        let (_cols, rows) = terminal::size()?;
        let height = rows as usize;

        queue!(stdout, cursor::MoveTo(0, 0))?;
        for row in 0..height {
            let idx = offset + row;
            if idx < lines.len() {
                write!(stdout, "{}", lines[idx].replace('\t', "    "))?;
            }
            queue!(stdout, terminal::Clear(ClearType::UntilNewLine))?;
            queue!(stdout, cursor::MoveTo(0, (row + 1) as u16))?;
        }

        let status = match (&query, match_line) {
            (Some(q), Some(n)) => format!("/{} (line {})", q, n + 1),
            (Some(q), None) => format!("No matches for \"{}\"", q),
            (None, _) => {
                let percent = if lines.is_empty() {
                    0
                } else {
                    (offset * 100) / lines.len()
                };
                format!("{} {}%", lines.len(), percent)
            }
        };
        queue!(stdout, cursor::MoveTo(0, height as u16), terminal::Clear(ClearType::CurrentLine))?;
        write!(stdout, "{}", status)?;
        stdout.flush()?;

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('/') => {
                    if let Some(q) = read_search_query()? {
                        query = Some(q);
                        match_line = find_forward(lines, query.as_deref().unwrap(), offset);
                        if let Some(n) = match_line {
                            offset = n;
                        }
                    }
                }
                KeyCode::Char('n') => {
                    if let Some(n) = match_line {
                        match_line = find_forward(lines, query.as_deref().unwrap(), n + 1);
                        if let Some(m) = match_line {
                            offset = m;
                        }
                    }
                }
                KeyCode::Char('N') => {
                    if let Some(n) = match_line {
                        let from = n as isize - 1;
                        match_line = find_backward(lines, query.as_deref().unwrap(), from);
                        if let Some(m) = match_line {
                            offset = m;
                        }
                    }
                }
                KeyCode::Char('q') if !key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char('j') | KeyCode::Down => offset = offset.saturating_add(1).min(max(lines, height)),
                KeyCode::Char('k') | KeyCode::Up => offset = offset.saturating_sub(1),
                KeyCode::Char(' ') | KeyCode::PageDown => offset = offset.saturating_add(height).min(max(lines, height)),
                KeyCode::Char('b') | KeyCode::PageUp => offset = offset.saturating_sub(height),
                KeyCode::Char('g') | KeyCode::Home => offset = 0,
                KeyCode::Char('G') | KeyCode::End => offset = max(lines, height),
                _ => {}
            },
            Event::Resize(..) => {}
            _ => {}
        }
    }
    Ok(())
}

/// Collect a `/` search query at the bottom of the screen.
/// Returns None if cancelled with Esc, Some(query) on Enter.
fn read_search_query() -> io::Result<Option<String>> {
    let mut stdout = io::stdout();
    let mut q = String::new();
    loop {
        let (_cols, rows) = terminal::size()?;
        let height = rows as usize;
        queue!(stdout, cursor::MoveTo(0, height as u16), terminal::Clear(ClearType::CurrentLine))?;
        write!(stdout, "/{}", q)?;
        stdout.flush()?;

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char(c) if !c.is_control() => q.push(c),
                KeyCode::Backspace => {
                    q.pop();
                }
                KeyCode::Esc => return Ok(None),
                KeyCode::Enter => return Ok(Some(q)),
                _ => {}
            },
            _ => {}
        }
    }
}

// Smart case: case-insensitive unless the query itself contains an uppercase letter.
fn make_matcher(query: &str) -> impl Fn(&str) -> bool {
    let insensitive = !query.chars().any(|c| c.is_uppercase());
    let folded = query.to_lowercase();
    move |hay| {
        if insensitive {
            // ponytail: allocates per line; fine for paging, revisit if scanning multi-MB files
            hay.to_lowercase().contains(&folded)
        } else {
            hay.contains(query)
        }
    }
}

fn find_forward(lines: &[&str], query: &str, mut from: usize) -> Option<usize> {
    if lines.is_empty() {
        return None;
    }
    if from >= lines.len() {
        from = 0;
    }
    let matcher = make_matcher(query);
    (from..lines.len()).chain(0..from).find(|&i| matcher(lines[i]))
}

fn find_backward(lines: &[&str], query: &str, from: isize) -> Option<usize> {
    if lines.is_empty() {
        return None;
    }
    let matcher = make_matcher(query);
    // Search downward from `from`, wrapping to the bottom when we pass the top.
    let count = lines.len();
    let i = if from < 0 { count - 1 } else { from as usize }; // inclusive start
    for step in 0..count {
        let idx = if i >= step { i - step } else { i + (count - step) };
        if matcher(lines[idx]) {
            return Some(idx);
        }
    }
    None
}

// Last valid offset keeps as much content on screen as possible without a blank screen.
fn max(lines: &[&str], height: usize) -> usize {
    lines.len().saturating_sub(height - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_clamps_and_never_underflows() {
        let five: Vec<&str> = vec!["h"; 5];
        assert_eq!(max(&five, 10), 0); // shorter than the screen
        assert_eq!(max(&five, 1), 5);
        let empty: Vec<&str> = vec![];
        assert_eq!(max(&empty, 24), 0);
    }

    #[test]
    fn smart_case_insensitive_for_lowercase_query() {
        let lines = vec!["Hello", "world", "HEY there"];
        assert!(make_matcher("hey")(lines[2])); // lowercase query matches uppercase text
        assert!(make_matcher("world")(lines[1]));
        assert!(make_matcher("Hello")(lines[0])); // lowercase fold still finds exact match
        assert_eq!(find_forward(&lines, "hey", 0), Some(2));
        assert_eq!(find_forward(&lines, "HeLlo", 0), None); // mixed case = exact only
    }

    #[test]
    fn smart_case_sensitive_for_uppercase_query() {
        let lines = vec!["abc", "ABC", "Abc"];
        assert_eq!(find_forward(&lines, "ABC", 0), Some(1));
        assert_eq!(find_forward(&lines, "Abc", 0), Some(2));
        assert_eq!(find_forward(&lines, "ABC", 0), Some(1));
    }

    #[test]
    fn find_wraps_around() {
        let lines = vec!["foo", "bar", "baz"];
        assert_eq!(find_forward(&lines, "foo", 2), Some(0)); // forward from end wraps to top
        assert_eq!(find_backward(&lines, "baz", 0), Some(2)); // backward from top wraps to bottom
    }

    #[test]
    fn find_empty_no_match() {
        let empty: Vec<&str> = vec![];
        assert_eq!(find_forward(&empty, "x", 0), None);
        assert_eq!(find_backward(&empty, "x", 0), None);
    }
}
