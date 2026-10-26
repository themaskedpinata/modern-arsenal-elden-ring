//! A small Valve KeyValues (text VDF) parser for `libraryfolders.vdf` and `appmanifest_<id>.acf`.
//!
//! Grammar handled: quoted or bare tokens, `{ }` blocks, `//` comments, the escapes `\\ \" \n \t` inside quoted
//! strings, and `[$WIN32]`-style conditionals after a value (skipped). Keys are kept as written; lookups are
//! case-insensitive (Steam writes both `installdir` and `InstallDir` over the years).

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Block(Vec<(String, Value)>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Block(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Value)] {
        match self {
            Value::Block(v) => v,
            Value::Str(_) => &[],
        }
    }

    /// First child named `k` (case-insensitive).
    pub fn get(&self, k: &str) -> Option<&Value> {
        self.entries().iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v)
    }

    pub fn get_str(&self, k: &str) -> Option<&str> {
        self.get(k).and_then(Value::as_str)
    }
}

#[derive(Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VDF line {}: {}", self.line, self.msg)
    }
}

#[derive(Debug, PartialEq)]
enum Tok {
    Str(String),
    Open,
    Close,
}

fn tokenize(src: &str) -> Result<Vec<(Tok, usize)>, ParseError> {
    let mut out = Vec::new();
    let mut it = src.trim_start_matches('\u{feff}').chars().peekable();
    let mut line = 1;
    while let Some(c) = it.next() {
        match c {
            '\n' => line += 1,
            c if c.is_whitespace() => {}
            '{' => out.push((Tok::Open, line)),
            '}' => out.push((Tok::Close, line)),
            '/' if it.peek() == Some(&'/') => {
                for c in it.by_ref() {
                    if c == '\n' {
                        line += 1;
                        break;
                    }
                }
            }
            '"' => {
                let start = line;
                let mut s = String::new();
                loop {
                    match it.next() {
                        None => return Err(ParseError { line: start, msg: "unterminated string".into() }),
                        Some('"') => break,
                        Some('\\') => match it.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some('\\') => s.push('\\'),
                            Some('"') => s.push('"'),
                            Some(o) => {
                                s.push('\\');
                                s.push(o);
                            }
                            None => return Err(ParseError { line: start, msg: "unterminated string".into() }),
                        },
                        Some('\n') => {
                            line += 1;
                            s.push('\n');
                        }
                        Some(o) => s.push(o),
                    }
                }
                out.push((Tok::Str(s), start));
            }
            '[' => {
                // conditional like [$WIN32]: skipped
                for c in it.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            }
            _ => {
                let mut s = String::from(c);
                while let Some(&n) = it.peek() {
                    if n.is_whitespace() || n == '{' || n == '}' || n == '"' {
                        break;
                    }
                    s.push(n);
                    it.next();
                }
                out.push((Tok::Str(s), line));
            }
        }
    }
    Ok(out)
}

fn parse_block(toks: &[(Tok, usize)], pos: &mut usize, nested: bool) -> Result<Vec<(String, Value)>, ParseError> {
    let mut out = Vec::new();
    while *pos < toks.len() {
        let (t, line) = &toks[*pos];
        match t {
            Tok::Close => {
                if !nested {
                    return Err(ParseError { line: *line, msg: "unexpected '}'".into() });
                }
                *pos += 1;
                return Ok(out);
            }
            Tok::Open => return Err(ParseError { line: *line, msg: "'{' without a key".into() }),
            Tok::Str(k) => {
                *pos += 1;
                match toks.get(*pos) {
                    Some((Tok::Str(v), _)) => {
                        *pos += 1;
                        out.push((k.clone(), Value::Str(v.clone())));
                    }
                    Some((Tok::Open, _)) => {
                        *pos += 1;
                        let b = parse_block(toks, pos, true)?;
                        out.push((k.clone(), Value::Block(b)));
                    }
                    _ => return Err(ParseError { line: *line, msg: format!("key '{k}' has no value") }),
                }
            }
        }
    }
    if nested {
        return Err(ParseError { line: toks.last().map(|t| t.1).unwrap_or(0), msg: "missing '}'".into() });
    }
    Ok(out)
}

/// Parses a whole document into a root block.
pub fn parse(src: &str) -> Result<Value, ParseError> {
    let toks = tokenize(src)?;
    let mut pos = 0;
    Ok(Value::Block(parse_block(&toks, &mut pos, false)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_folders_modern() {
        let src = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"228980\"\t\t\"639714529\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"1245620\"\t\t\"55123268094\"\n\t\t\t\"10180\"\t\"1\"\n\t\t}\n\t}\n}\n";
        let v = parse(src).unwrap();
        let lf = v.get("LibraryFolders").unwrap();
        assert_eq!(lf.entries().len(), 2);
        assert_eq!(lf.get("1").unwrap().get_str("path"), Some(r"D:\SteamLibrary"));
        let apps: Vec<&str> = lf.get("1").unwrap().get("apps").unwrap().entries().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(apps, vec!["1245620", "10180"]);
    }

    #[test]
    fn comments_bare_tokens_conditionals_bom() {
        let v = parse("\u{feff}// header\nAppState { appid 10180 // trailing\n \"name\" \"a \\\"q\\\" b\" [$WIN32]\n }").unwrap();
        let a = v.get("appstate").unwrap();
        assert_eq!(a.get_str("APPID"), Some("10180"));
        assert_eq!(a.get_str("name"), Some("a \"q\" b"));
    }

    #[test]
    fn errors_have_lines() {
        assert_eq!(parse("\"a\" {\n\"b\" \"c\"\n").unwrap_err().msg, "missing '}'");
        assert_eq!(parse("\"a\"").unwrap_err().msg, "key 'a' has no value");
        assert_eq!(parse("}").unwrap_err().line, 1);
        assert!(parse("\"a\" \"b").is_err());
        assert_eq!(parse("").unwrap(), Value::Block(vec![]));
    }
}
