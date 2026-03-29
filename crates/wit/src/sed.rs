use anyhow::{Result, bail};
use regex::{Regex, RegexBuilder};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};

#[derive(Debug, Clone, Default)]
pub struct SedOptions {
    pub quiet: bool,
}

#[derive(Debug)]
pub struct SedOutput {
    pub output: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone)]
pub struct SedProgram {
    commands: Vec<Command>,
    labels: HashMap<String, usize>,
    blocks: HashMap<usize, usize>,
}

#[derive(Debug, Clone)]
struct Command {
    address: Option<AddressRange>,
    negate: bool,
    kind: CommandKind,
}

#[derive(Debug, Clone)]
struct AddressRange {
    start: Address,
    end: Option<Address>,
}

#[derive(Debug, Clone)]
enum Address {
    Line(usize),
    Last,
    Regex(Regex),
}

#[derive(Debug, Clone)]
enum CommandKind {
    Print,
    PrintFirst,
    Delete,
    DeleteFirst,
    Quit(Option<i32>),
    Substitute(Substitute),
    Append(String),
    Insert(String),
    Change(String),
    Read(String),
    Write(String),
    Translate(Translate),
    GetHold,
    AppendHold,
    SetHold,
    AddHold,
    Exchange,
    Next,
    NextAppend,
    PrintLineNumber,
    List,
    Branch(Option<String>),
    Test(Option<String>),
    Label(String),
    Comment,
    BlockStart,
    BlockEnd,
}

#[derive(Debug, Clone)]
struct Substitute {
    regex: Regex,
    replacement: Vec<ReplacementToken>,
    global: bool,
    print: bool,
    write: Option<String>,
    occurrence: Option<usize>,
}

#[derive(Debug, Clone)]
struct Translate {
    from: Vec<char>,
    to: Vec<char>,
}

#[derive(Debug, Clone)]
enum ReplacementToken {
    Literal(String),
    Group(usize),
    WholeMatch,
}

pub fn parse_script(sources: &[String]) -> Result<SedProgram> {
    if sources.is_empty() {
        bail!("no sed script provided");
    }

    let mut script = String::new();
    for (idx, source) in sources.iter().enumerate() {
        if idx > 0 && !script.ends_with('\n') {
            script.push('\n');
        }
        script.push_str(source);
    }

    let mut parser = Parser::new(&script);
    let commands = parser.parse_commands()?;
    let mut labels = HashMap::new();

    for (idx, command) in commands.iter().enumerate() {
        if let CommandKind::Label(label) = &command.kind {
            if labels.contains_key(label) {
                bail!("duplicate label: {label}");
            }
            labels.insert(label.clone(), idx);
        }
    }

    let mut blocks = HashMap::new();
    let mut block_stack = Vec::new();

    for (idx, command) in commands.iter().enumerate() {
        match command.kind {
            CommandKind::BlockStart => block_stack.push(idx),
            CommandKind::BlockEnd => {
                let start = block_stack
                    .pop()
                    .ok_or_else(|| anyhow::anyhow!("unmatched '}}' at command {idx}"))?;
                blocks.insert(start, idx);
                blocks.insert(idx, start);
            }
            _ => {}
        }
    }

    if !block_stack.is_empty() {
        bail!("unmatched '{{' in script");
    }

    Ok(SedProgram {
        commands,
        labels,
        blocks,
    })
}

pub fn run(program: &SedProgram, input: &str, options: &SedOptions) -> Result<SedOutput> {
    let lines: Vec<String> = if input.is_empty() {
        Vec::new()
    } else {
        input.lines().map(|line| line.to_string()).collect()
    };

    let mut runtime = SedRuntime::new(program, options.quiet);
    let exit_code = runtime.execute(&lines)?;

    Ok(SedOutput {
        output: runtime.output,
        exit_code,
    })
}

struct SedRuntime<'a> {
    program: &'a SedProgram,
    quiet: bool,
    range_active: Vec<bool>,
    substituted: bool,
    output: String,
    hold_space: String,
    writers: HashMap<String, BufWriter<std::fs::File>>,
}

impl<'a> SedRuntime<'a> {
    fn new(program: &'a SedProgram, quiet: bool) -> Self {
        Self {
            program,
            quiet,
            range_active: vec![false; program.commands.len()],
            substituted: false,
            output: String::new(),
            hold_space: String::new(),
            writers: HashMap::new(),
        }
    }

    fn execute(&mut self, lines: &[String]) -> Result<i32> {
        let mut next_idx = 0usize;
        let total_lines = lines.len();
        let mut exit_code = 0i32;

        while next_idx < total_lines {
            let mut pattern = lines[next_idx].clone();
            next_idx += 1;
            let mut current_line_num = next_idx;
            let mut append_queue: Vec<String> = Vec::new();
            let mut auto_print = !self.quiet;
            self.substituted = false;

            'cycle: loop {
                let mut cmd_index = 0usize;
                let mut restart_cycle = false;
                let mut print_at_end = true;

                while cmd_index < self.program.commands.len() {
                    let command = &self.program.commands[cmd_index];

                    if matches!(command.kind, CommandKind::BlockEnd) {
                        cmd_index += 1;
                        continue;
                    }

                    if matches!(command.kind, CommandKind::BlockStart) {
                        let applies = self.command_applies(
                            command,
                            &pattern,
                            current_line_num,
                            total_lines,
                            cmd_index,
                        )?;
                        if !applies {
                            if let Some(end_idx) = self.program.blocks.get(&cmd_index) {
                                cmd_index = end_idx + 1;
                                continue;
                            }
                            bail!("block start without matching end");
                        }
                        cmd_index += 1;
                        continue;
                    }

                    if !self.command_applies(
                        command,
                        &pattern,
                        current_line_num,
                        total_lines,
                        cmd_index,
                    )? {
                        cmd_index += 1;
                        continue;
                    }

                    match &command.kind {
                        CommandKind::Print => {
                            self.print_pattern(&pattern);
                        }
                        CommandKind::PrintFirst => {
                            let first = pattern.split('\n').next().unwrap_or("");
                            self.print_text(first);
                        }
                        CommandKind::Delete => {
                            self.flush_append(&mut append_queue);
                            print_at_end = false;
                            break;
                        }
                        CommandKind::DeleteFirst => {
                            if let Some(pos) = pattern.find('\n') {
                                pattern = pattern[(pos + 1)..].to_string();
                                self.flush_append(&mut append_queue);
                                append_queue.clear();
                                auto_print = !self.quiet;
                                self.substituted = false;
                                restart_cycle = true;
                                print_at_end = false;
                                break;
                            } else {
                                self.flush_append(&mut append_queue);
                                print_at_end = false;
                                break;
                            }
                        }
                        CommandKind::Quit(code) => {
                            if auto_print {
                                self.print_pattern(&pattern);
                            }
                            self.flush_append(&mut append_queue);
                            exit_code = code.unwrap_or(0);
                            return Ok(exit_code);
                        }
                        CommandKind::Substitute(subst) => {
                            let (new_pattern, did_substitute, did_print) =
                                self.apply_substitute(&pattern, subst)?;
                            if did_substitute {
                                pattern = new_pattern;
                                self.substituted = true;
                                if did_print {
                                    self.print_pattern(&pattern);
                                }
                                if let Some(path) = &subst.write {
                                    self.write_to_file(path, &pattern)?;
                                }
                            }
                        }
                        CommandKind::Append(text) => {
                            self.enqueue_text(&mut append_queue, text);
                        }
                        CommandKind::Insert(text) => {
                            for line in text.split('\n') {
                                self.print_text(line);
                            }
                        }
                        CommandKind::Change(text) => {
                            for line in text.split('\n') {
                                self.print_text(line);
                            }
                            self.flush_append(&mut append_queue);
                            print_at_end = false;
                            break;
                        }
                        CommandKind::Read(path) => {
                            let contents = std::fs::read_to_string(path).map_err(|e| {
                                anyhow::anyhow!("failed to read file for r command: {e}")
                            })?;
                            for line in contents.lines() {
                                append_queue.push(line.to_string());
                            }
                        }
                        CommandKind::Write(path) => {
                            self.write_to_file(path, &pattern)?;
                        }
                        CommandKind::Translate(map) => {
                            pattern = translate_pattern(&pattern, map);
                        }
                        CommandKind::GetHold => {
                            pattern = self.hold_space.clone();
                        }
                        CommandKind::AppendHold => {
                            if !self.hold_space.is_empty() {
                                if !pattern.is_empty() {
                                    pattern.push('\n');
                                }
                                pattern.push_str(&self.hold_space);
                            }
                        }
                        CommandKind::SetHold => {
                            self.hold_space = pattern.clone();
                        }
                        CommandKind::AddHold => {
                            if !self.hold_space.is_empty() {
                                self.hold_space.push('\n');
                            }
                            self.hold_space.push_str(&pattern);
                        }
                        CommandKind::Exchange => {
                            std::mem::swap(&mut pattern, &mut self.hold_space);
                        }
                        CommandKind::Next => {
                            if auto_print {
                                self.print_pattern(&pattern);
                            }
                            self.flush_append(&mut append_queue);
                            if next_idx >= total_lines {
                                return Ok(exit_code);
                            }
                            pattern = lines[next_idx].clone();
                            next_idx += 1;
                            current_line_num = next_idx;
                            append_queue.clear();
                            auto_print = !self.quiet;
                            self.substituted = false;
                            restart_cycle = true;
                            print_at_end = false;
                            break;
                        }
                        CommandKind::NextAppend => {
                            if next_idx >= total_lines {
                                if auto_print {
                                    self.print_pattern(&pattern);
                                }
                                self.flush_append(&mut append_queue);
                                return Ok(exit_code);
                            }
                            pattern.push('\n');
                            pattern.push_str(&lines[next_idx]);
                            next_idx += 1;
                            current_line_num = next_idx;
                        }
                        CommandKind::PrintLineNumber => {
                            self.print_text(&current_line_num.to_string());
                        }
                        CommandKind::List => {
                            let listed = list_pattern(&pattern);
                            self.print_text(&listed);
                        }
                        CommandKind::Branch(label) => {
                            if let Some(label) = label {
                                if let Some(target) = self.program.labels.get(label) {
                                    cmd_index = *target;
                                    continue;
                                }
                                bail!("unknown label: {label}");
                            } else {
                                cmd_index = self.program.commands.len();
                                continue;
                            }
                        }
                        CommandKind::Test(label) => {
                            if self.substituted {
                                self.substituted = false;
                                if let Some(label) = label {
                                    if let Some(target) = self.program.labels.get(label) {
                                        cmd_index = *target;
                                        continue;
                                    }
                                    bail!("unknown label: {label}");
                                } else {
                                    cmd_index = self.program.commands.len();
                                    continue;
                                }
                            }
                        }
                        CommandKind::Label(_) | CommandKind::Comment => {}
                        CommandKind::BlockStart | CommandKind::BlockEnd => {}
                    }

                    cmd_index += 1;
                }

                if restart_cycle {
                    continue 'cycle;
                }

                if print_at_end && auto_print {
                    self.print_pattern(&pattern);
                }
                self.flush_append(&mut append_queue);
                break 'cycle;
            }
        }

        Ok(exit_code)
    }

    fn command_applies(
        &mut self,
        command: &Command,
        pattern: &str,
        line_num: usize,
        total_lines: usize,
        cmd_index: usize,
    ) -> Result<bool> {
        let mut applies = match &command.address {
            None => true,
            Some(range) => match &range.end {
                None => self.address_matches(&range.start, pattern, line_num, total_lines),
                Some(_end) => {
                    let active = self.range_active.get(cmd_index).copied().unwrap_or(false);
                    if !active {
                        if self.address_matches(&range.start, pattern, line_num, total_lines) {
                            self.range_active[cmd_index] = true;
                            true
                        } else {
                            false
                        }
                    } else {
                        true
                    }
                }
            },
        };

        if let Some(range) = &command.address
            && range.end.is_some()
        {
            let active = self.range_active.get(cmd_index).copied().unwrap_or(false);
            if active
                && self.address_matches(range.end.as_ref().unwrap(), pattern, line_num, total_lines)
            {
                self.range_active[cmd_index] = false;
            }
        }

        if command.negate {
            applies = !applies;
        }
        Ok(applies)
    }

    fn address_matches(
        &self,
        address: &Address,
        pattern: &str,
        line_num: usize,
        total_lines: usize,
    ) -> bool {
        match address {
            Address::Line(target) => line_num == *target,
            Address::Last => line_num == total_lines,
            Address::Regex(regex) => regex.is_match(pattern),
        }
    }

    fn apply_substitute(&self, pattern: &str, subst: &Substitute) -> Result<(String, bool, bool)> {
        let mut result = String::new();
        let mut last_end = 0usize;
        let mut match_index = 0usize;
        let mut replaced = false;
        for caps in subst.regex.captures_iter(pattern) {
            let m = caps.get(0).unwrap();
            match_index += 1;
            let should_replace = match subst.occurrence {
                Some(target) => match_index == target,
                None => {
                    if subst.global {
                        true
                    } else {
                        !replaced
                    }
                }
            };

            if should_replace {
                result.push_str(&pattern[last_end..m.start()]);
                result.push_str(&apply_replacement(&subst.replacement, &caps));
                last_end = m.end();
                replaced = true;
                if subst.occurrence.is_some() && match_index == subst.occurrence.unwrap() {
                    break;
                }
            } else if subst.occurrence.is_some() && match_index > subst.occurrence.unwrap() {
                break;
            }
        }

        if replaced {
            result.push_str(&pattern[last_end..]);
            Ok((result, true, subst.print))
        } else {
            Ok((pattern.to_string(), false, false))
        }
    }

    fn enqueue_text(&self, queue: &mut Vec<String>, text: &str) {
        for line in text.split('\n') {
            queue.push(line.to_string());
        }
    }

    fn print_pattern(&mut self, pattern: &str) {
        self.output.push_str(pattern);
        self.output.push('\n');
    }

    fn print_text(&mut self, text: &str) {
        self.output.push_str(text);
        self.output.push('\n');
    }

    fn flush_append(&mut self, queue: &mut Vec<String>) {
        for line in queue.drain(..) {
            self.output.push_str(&line);
            self.output.push('\n');
        }
    }

    fn write_to_file(&mut self, path: &str, pattern: &str) -> Result<()> {
        let writer = self.writers.entry(path.to_string()).or_insert_with(|| {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("failed to open file for w command");
            BufWriter::new(file)
        });
        writeln!(writer, "{}", pattern)?;
        Ok(())
    }
}

fn translate_pattern(pattern: &str, map: &Translate) -> String {
    pattern
        .chars()
        .map(|ch| {
            if let Some(idx) = map.from.iter().position(|c| *c == ch) {
                map.to.get(idx).copied().unwrap_or(ch)
            } else {
                ch
            }
        })
        .collect()
}

fn list_pattern(pattern: &str) -> String {
    let mut out = String::new();
    for ch in pattern.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('$');
    out
}

fn apply_replacement(tokens: &[ReplacementToken], caps: &regex::Captures<'_>) -> String {
    let mut out = String::new();
    for token in tokens {
        match token {
            ReplacementToken::Literal(text) => out.push_str(text),
            ReplacementToken::WholeMatch => {
                if let Some(m) = caps.get(0) {
                    out.push_str(m.as_str());
                }
            }
            ReplacementToken::Group(idx) => {
                if let Some(m) = caps.get(*idx) {
                    out.push_str(m.as_str());
                }
            }
        }
    }
    out
}

struct Parser {
    chars: Vec<char>,
    idx: usize,
    line: usize,
    col: usize,
    last_regex: Option<String>,
}

impl Parser {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            idx: 0,
            line: 1,
            col: 1,
            last_regex: None,
        }
    }

    fn parse_commands(&mut self) -> Result<Vec<Command>> {
        let mut commands = Vec::new();
        self.skip_separators();

        while !self.eof() {
            if self.peek() == Some('#') {
                self.consume_line();
                self.skip_separators();
                continue;
            }

            let addr1 = self.parse_address()?;
            let addr2 = if addr1.is_some() {
                self.skip_spaces();
                if self.peek() == Some(',') {
                    self.advance();
                    Some(self.parse_address()?.ok_or_else(|| {
                        anyhow::anyhow!("expected address after ',' at line {}", self.line)
                    })?)
                } else {
                    None
                }
            } else {
                None
            };

            let address = addr1.map(|start| AddressRange { start, end: addr2 });

            self.skip_spaces();
            let negate = if self.peek() == Some('!') {
                self.advance();
                true
            } else {
                false
            };

            self.skip_spaces();
            let command_char = self
                .advance()
                .ok_or_else(|| anyhow::anyhow!("expected command at line {}", self.line))?;

            let kind = match command_char {
                'p' => CommandKind::Print,
                'P' => CommandKind::PrintFirst,
                'd' => CommandKind::Delete,
                'D' => CommandKind::DeleteFirst,
                'q' => {
                    let num = self.parse_optional_number();
                    CommandKind::Quit(num)
                }
                's' => CommandKind::Substitute(self.parse_substitute()?),
                'a' => CommandKind::Append(self.parse_text_block()?),
                'i' => CommandKind::Insert(self.parse_text_block()?),
                'c' => CommandKind::Change(self.parse_text_block()?),
                'r' => CommandKind::Read(self.parse_filename()?),
                'w' => CommandKind::Write(self.parse_filename()?),
                'y' => CommandKind::Translate(self.parse_translate()?),
                'g' => CommandKind::GetHold,
                'G' => CommandKind::AppendHold,
                'h' => CommandKind::SetHold,
                'H' => CommandKind::AddHold,
                'x' => CommandKind::Exchange,
                'n' => CommandKind::Next,
                'N' => CommandKind::NextAppend,
                '=' => CommandKind::PrintLineNumber,
                'l' => CommandKind::List,
                'b' => CommandKind::Branch(self.parse_label_ref()),
                't' => CommandKind::Test(self.parse_label_ref()),
                ':' => {
                    let label = self.parse_label()?;
                    if label.is_empty() {
                        bail!("label required after ':' at line {}", self.line);
                    }
                    CommandKind::Label(label)
                }
                '#' => {
                    self.consume_line();
                    CommandKind::Comment
                }
                '{' => CommandKind::BlockStart,
                '}' => CommandKind::BlockEnd,
                _ => {
                    bail!(
                        "unsupported command '{}' at line {}",
                        command_char,
                        self.line
                    )
                }
            };

            commands.push(Command {
                address,
                negate,
                kind,
            });

            self.skip_separators();
        }

        Ok(commands)
    }

    fn parse_address(&mut self) -> Result<Option<Address>> {
        self.skip_spaces();
        let Some(ch) = self.peek() else {
            return Ok(None);
        };
        if ch.is_ascii_digit() {
            let number = self.parse_number()?;
            return Ok(Some(Address::Line(number)));
        }
        if ch == '$' {
            self.advance();
            return Ok(Some(Address::Last));
        }
        if ch == '/' {
            let regex = self.parse_regex('/')?;
            return Ok(Some(Address::Regex(regex)));
        }
        Ok(None)
    }

    fn parse_substitute(&mut self) -> Result<Substitute> {
        let delim = self
            .advance()
            .ok_or_else(|| anyhow::anyhow!("missing delimiter for s"))?;
        let pattern_text = self.parse_delimited(delim)?;
        let pattern = if pattern_text.is_empty() {
            self.last_regex
                .clone()
                .ok_or_else(|| anyhow::anyhow!("empty regex with no previous pattern"))?
        } else {
            self.last_regex = Some(pattern_text.clone());
            pattern_text
        };

        let replacement_raw = self.parse_delimited(delim)?;
        let mut global = false;
        let mut print = false;
        let mut write: Option<String> = None;
        let mut occurrence: Option<usize> = None;
        let mut case_insensitive = false;

        loop {
            match self.peek() {
                Some('g') => {
                    global = true;
                    self.advance();
                }
                Some('p') => {
                    print = true;
                    self.advance();
                }
                Some('i') | Some('I') => {
                    case_insensitive = true;
                    self.advance();
                }
                Some('w') => {
                    self.advance();
                    let filename = self.parse_filename()?;
                    if filename.is_empty() {
                        bail!("missing filename for w flag");
                    }
                    write = Some(filename);
                }
                Some(ch) if ch.is_ascii_digit() => {
                    let number = self.parse_number()?;
                    occurrence = Some(number);
                }
                _ => break,
            }
        }

        let mut builder = RegexBuilder::new(&pattern);
        builder.case_insensitive(case_insensitive);
        let regex = builder
            .build()
            .map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;

        let replacement = parse_replacement(&replacement_raw, delim)?;

        Ok(Substitute {
            regex,
            replacement,
            global,
            print,
            write,
            occurrence,
        })
    }

    fn parse_translate(&mut self) -> Result<Translate> {
        let delim = self
            .advance()
            .ok_or_else(|| anyhow::anyhow!("missing delimiter for y"))?;
        let from_raw = self.parse_delimited(delim)?;
        let to_raw = self.parse_delimited(delim)?;
        let from = unescape_basic(&from_raw, delim)?;
        let to = unescape_basic(&to_raw, delim)?;
        if from.len() != to.len() {
            bail!("y command requires equal-length strings");
        }
        Ok(Translate {
            from: from.chars().collect(),
            to: to.chars().collect(),
        })
    }

    fn parse_text_block(&mut self) -> Result<String> {
        self.skip_spaces();
        if self.peek() == Some('\\') {
            self.advance();
            if self.peek() == Some('\n') {
                self.advance();
            }
        }

        let mut text = String::new();
        loop {
            let line = self.consume_line();
            let mut backslash_count = 0usize;
            for ch in line.chars().rev() {
                if ch == '\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            if backslash_count % 2 == 1 {
                text.push_str(&line[..line.len() - 1]);
                text.push('\n');
                continue;
            }
            text.push_str(&line);
            break;
        }
        Ok(unescape_text(&text))
    }

    fn parse_filename(&mut self) -> Result<String> {
        self.skip_spaces();
        let mut name = String::new();
        while let Some(ch) = self.peek() {
            if ch == '\n' || ch == ';' {
                break;
            }
            name.push(ch);
            self.advance();
        }
        Ok(name.trim().to_string())
    }

    fn parse_label_ref(&mut self) -> Option<String> {
        self.skip_spaces();
        let label = self.parse_label().ok()?;
        if label.is_empty() { None } else { Some(label) }
    }

    fn parse_label(&mut self) -> Result<String> {
        self.skip_spaces();
        let mut label = String::new();
        while let Some(ch) = self.peek() {
            if ch == '\n' || ch == ';' || ch.is_whitespace() {
                break;
            }
            label.push(ch);
            self.advance();
        }
        Ok(label)
    }

    fn parse_number(&mut self) -> Result<usize> {
        let mut number = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                number.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        if number.is_empty() {
            bail!("expected number at line {}", self.line);
        }
        Ok(number.parse::<usize>()?)
    }

    fn parse_optional_number(&mut self) -> Option<i32> {
        self.skip_spaces();
        let mut number = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                number.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        if number.is_empty() {
            None
        } else {
            number.parse::<i32>().ok()
        }
    }

    fn parse_regex(&mut self, delim: char) -> Result<Regex> {
        if self.peek() == Some(delim) {
            self.advance();
        } else {
            bail!("expected regex delimiter at line {}", self.line);
        }
        let pattern = self.parse_delimited(delim)?;
        let pattern = if pattern.is_empty() {
            self.last_regex
                .clone()
                .ok_or_else(|| anyhow::anyhow!("empty regex with no previous pattern"))?
        } else {
            self.last_regex = Some(pattern.clone());
            pattern
        };
        let regex = Regex::new(&pattern).map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;
        Ok(regex)
    }

    fn parse_delimited(&mut self, delim: char) -> Result<String> {
        let mut out = String::new();
        while let Some(ch) = self.peek() {
            self.advance();
            if ch == delim {
                return Ok(out);
            }
            if ch == '\\' {
                if let Some(next) = self.peek() {
                    self.advance();
                    if next == delim {
                        out.push(delim);
                    } else {
                        out.push('\\');
                        out.push(next);
                    }
                } else {
                    out.push('\\');
                }
            } else {
                out.push(ch);
            }
        }
        bail!("unterminated delimiter at line {}", self.line)
    }

    fn skip_spaces(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == ' ' || ch == '\t' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_separators(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == ' ' || ch == '\t' || ch == '\n' || ch == ';' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn consume_line(&mut self) -> String {
        let mut line = String::new();
        while let Some(ch) = self.peek() {
            if ch == '\n' {
                self.advance();
                break;
            }
            line.push(ch);
            self.advance();
        }
        line
    }

    fn eof(&self) -> bool {
        self.idx >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.idx).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.chars.get(self.idx).copied();
        if let Some(c) = ch {
            if c == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            self.idx += 1;
        }
        ch
    }
}

fn parse_replacement(raw: &str, delim: char) -> Result<Vec<ReplacementToken>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '&' {
            if !current.is_empty() {
                tokens.push(ReplacementToken::Literal(current.clone()));
                current.clear();
            }
            tokens.push(ReplacementToken::WholeMatch);
            continue;
        }
        if ch == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    '0'..='9' => {
                        if !current.is_empty() {
                            tokens.push(ReplacementToken::Literal(current.clone()));
                            current.clear();
                        }
                        let idx = next.to_digit(10).unwrap() as usize;
                        tokens.push(ReplacementToken::Group(idx));
                    }
                    'n' => current.push('\n'),
                    't' => current.push('\t'),
                    '\\' => current.push('\\'),
                    '&' => current.push('&'),
                    _ if next == delim => current.push(delim),
                    other => current.push(other),
                }
            } else {
                current.push('\\');
            }
            continue;
        }
        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(ReplacementToken::Literal(current));
    }

    Ok(tokens)
}

fn unescape_basic(raw: &str, delim: char) -> Result<String> {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(match next {
                    'n' => '\n',
                    't' => '\t',
                    _ if next == delim => delim,
                    other => other,
                });
            } else {
                out.push('\\');
            }
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

fn unescape_text(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '\\' => out.push('\\'),
                    other => out.push(other),
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn run_script(script: &str, input: &str, quiet: bool) -> String {
        let program = parse_script(&[script.to_string()]).unwrap();
        let output = run(&program, input, &SedOptions { quiet }).unwrap();
        output.output
    }

    #[test]
    fn test_delete_line() {
        let input = "alpha\nbeta\ngamma\n";
        let output = run_script("2d", input, false);
        assert_eq!(output, "alpha\ngamma\n");
    }

    #[test]
    fn test_print_range_quiet() {
        let input = "alpha\nbeta\ngamma\ndelta\n";
        let output = run_script("2,3p", input, true);
        assert_eq!(output, "beta\ngamma\n");
    }

    #[test]
    fn test_regex_range() {
        let input = "alpha\nbeta\ngamma\ndelta\n";
        let output = run_script("/beta/,/delta/p", input, true);
        assert_eq!(output, "beta\ngamma\ndelta\n");
    }

    #[test]
    fn test_negation() {
        let input = "alpha\nbeta\ngamma\n";
        let output = run_script("1!p", input, true);
        assert_eq!(output, "beta\ngamma\n");
    }

    #[test]
    fn test_substitute_basic() {
        let input = "alpha\nbeta\n";
        let output = run_script("s/a/A/", input, false);
        assert_eq!(output, "Alpha\nbetA\n");
    }

    #[test]
    fn test_substitute_global_and_print() {
        let input = "banana\n";
        let output = run_script("s/a/A/gp", input, true);
        assert_eq!(output, "bAnAnA\n");
    }

    #[test]
    fn test_substitute_nth() {
        let input = "banana\n";
        let output = run_script("s/a/A/2p", input, true);
        assert_eq!(output, "banAna\n");
    }

    #[test]
    fn test_substitute_groups_and_whole() {
        let input = "abc123\n";
        let output = run_script("s/(abc)(123)/\\2-\\1-&/", input, false);
        assert_eq!(output, "123-abc-abc123\n");
    }

    #[test]
    fn test_translate() {
        let input = "abc cab\n";
        let output = run_script("y/abc/xyz/", input, false);
        assert_eq!(output, "xyz zxy\n");
    }

    #[test]
    fn test_insert_and_append() {
        let input = "alpha\n";
        let output = run_script("i\\BEFORE\np\na\\AFTER", input, true);
        assert_eq!(output, "BEFORE\nalpha\nAFTER\n");
    }

    #[test]
    fn test_change_command() {
        let input = "alpha\nbeta\n";
        let output = run_script("1c\\X", input, false);
        assert_eq!(output, "X\nbeta\n");
    }

    #[test]
    fn test_hold_and_exchange() {
        let input = "alpha\n";
        let output = run_script("h;s/.*/X/;x;p", input, true);
        assert_eq!(output, "alpha\n");
    }

    #[test]
    fn test_hold_append() {
        let input = "one\ntwo\n";
        let output = run_script("1h;2{H;G;p}", input, true);
        assert_eq!(output, "two\none\ntwo\n");
    }

    #[test]
    fn test_branch_and_test() {
        let input = "aaa\n";
        let output = run_script(":a;s/a/A/;ta;p", input, true);
        assert_eq!(output, "AAA\n");
    }

    #[test]
    fn test_block_address() {
        let input = "alpha\nbeta\n";
        let output = run_script("1,1{ s/a/A/; p }", input, true);
        assert_eq!(output, "Alpha\n");
    }

    #[test]
    fn test_print_line_number() {
        let input = "alpha\n";
        let output = run_script("=;p", input, true);
        assert_eq!(output, "1\nalpha\n");
    }

    #[test]
    fn test_list_command() {
        let input = "a\tb\n";
        let output = run_script("l", input, true);
        assert_eq!(output, "a\\tb$\n");
    }

    #[test]
    fn test_read_command() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "EXTRA").unwrap();
        let path = file.path().to_string_lossy().to_string();

        let input = "alpha\n";
        let output = run_script(&format!("r {}", path), input, true);
        assert_eq!(output, "EXTRA\n");
    }

    #[test]
    fn test_write_command() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_string_lossy().to_string();

        let input = "alpha\nbeta\n";
        let output = run_script(&format!("w {}", path), input, true);
        assert_eq!(output, "");

        let contents = std::fs::read_to_string(path).unwrap();
        assert_eq!(contents, "alpha\nbeta\n");
    }

    #[test]
    fn test_delete_first_no_newline() {
        let input = "alpha\n";
        let output = run_script("D;p", input, true);
        assert_eq!(output, "");
    }

    #[test]
    fn test_delete_first_multiline() {
        let input = "1\n2\n3\n";
        let output = run_script("N;P;D", input, true);
        assert_eq!(output, "1\n2\n");
    }

    #[test]
    fn test_empty_regex_reuse() {
        let input = "alpha\nbeta\n";
        let output = run_script("/alpha/p;//p", input, true);
        assert_eq!(output, "alpha\nalpha\n");
    }

    #[test]
    fn test_substitute_empty_pattern_reuse() {
        let input = "alpha\n";
        let output = run_script("/alpha/ s//ALPHA/", input, false);
        assert_eq!(output, "ALPHA\n");
    }

    #[test]
    fn test_negated_range() {
        let input = "a\nb\nc\n";
        let output = run_script("1,2!p", input, true);
        assert_eq!(output, "c\n");
    }

    #[test]
    fn test_unmatched_block_fails() {
        let result = parse_script(&["{ p".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_translate_length_mismatch() {
        let result = parse_script(&["y/ab/xyz/".to_string()]);
        assert!(result.is_err());
    }
}
