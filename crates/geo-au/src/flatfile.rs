//! Minimal reader for the G-NAF flat-file format: pipe-separated values
//! (`|`) with an uppercase header row and RFC 4180-style double-quoting.
//!
//! Column access is header-driven, so a schema change surfaces as a
//! [`MissingColumn`] error naming the file and the headers it did find
//! rather than silently shifting columns.

use std::io::{self, BufRead};
use std::path::Path;

use thiserror::Error;

/// A required column is absent from a file's header row.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("file {file:?} has no column '{column}' (found headers: {headers})")]
pub struct MissingColumn {
    pub file: String,
    pub column: String,
    pub headers: String,
}

/// A pipe-separated G-NAF flat file with a header row.
#[derive(Debug)]
pub struct PsvReader<R> {
    reader: R,
    file: String,
    headers: Vec<String>,
    line: String,
}

impl PsvReader<std::io::BufReader<std::fs::File>> {
    /// Opens `path` for reading and parses its header row.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        Self::from_reader(path.display().to_string(), io::BufReader::new(file))
    }
}

impl<R: BufRead> PsvReader<R> {
    /// Reads the header row from `reader`, naming it `file` in errors.
    pub fn from_reader(file: String, mut reader: R) -> io::Result<Self> {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let headers = split_line(strip_bom(&line))
            .into_iter()
            .map(|field| field.trim().to_ascii_uppercase())
            .collect();
        Ok(Self {
            reader,
            file,
            headers,
            line: String::new(),
        })
    }

    /// The upper-cased header names.
    #[must_use]
    pub fn headers(&self) -> &[String] {
        &self.headers
    }

    /// Index of a column (upper-case, exact).
    #[must_use]
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.headers.iter().position(|header| header == name)
    }

    /// Index of a required column.
    pub fn required(&self, name: &str) -> Result<usize, MissingColumn> {
        self.index_of(name).ok_or_else(|| MissingColumn {
            file: self.file.clone(),
            column: name.to_string(),
            headers: self.headers.join(", "),
        })
    }

    /// The file's display name, for context in errors.
    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    /// The next row, or `None` at end of file. Blank lines are skipped;
    /// rows may have fewer fields than headers (trailing empty values are
    /// common in G-NAF exports).
    pub fn next_row(&mut self) -> io::Result<Option<Vec<String>>> {
        loop {
            self.line.clear();
            if self.reader.read_line(&mut self.line)? == 0 {
                return Ok(None);
            }
            let line = self.line.trim_end_matches(['\r', '\n']);
            if line.trim().is_empty() {
                continue;
            }
            return Ok(Some(split_line(line)));
        }
    }
}

/// Splits one PSV line, honouring double quotes.
///
/// A `"` at field position toggles quoting; `""` inside a quoted field is a
/// literal quote. The delimiter is `|`.
#[must_use]
pub fn split_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if in_quotes {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(character);
            }
        } else {
            match character {
                '"' => in_quotes = true,
                '|' => fields.push(std::mem::take(&mut current)),
                _ => current.push(character),
            }
        }
    }
    fields.push(current);
    fields
}

/// Removes a UTF-8 byte-order mark from the first line.
fn strip_bom(line: &str) -> &str {
    line.strip_prefix('\u{feff}').unwrap_or(line)
}

/// Reads a field as an unsigned number, tolerating empty and padded values.
#[must_use]
pub fn parse_u32(field: Option<&String>) -> Option<u32> {
    field.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            trimmed.parse().ok()
        }
    })
}

/// Reads a field as `f64`, tolerating empty values.
#[must_use]
pub fn parse_f64(field: Option<&String>) -> Option<f64> {
    field.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            trimmed.parse().ok()
        }
    })
}

/// Reads a field, returning `None` for empty strings.
#[must_use]
pub fn parse_text(field: Option<&String>) -> Option<String> {
    field.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_plain_rows() {
        assert_eq!(split_line("a|b|c"), vec!["a", "b", "c"]);
        assert_eq!(split_line("a||c"), vec!["a", "", "c"]);
        assert_eq!(split_line(""), vec![""]);
    }

    #[test]
    fn handles_quotes_and_escaped_quotes() {
        assert_eq!(
            split_line("\"Smith|Street\"|12|\"say \"\"hi\"\"\""),
            vec!["Smith|Street", "12", "say \"hi\""]
        );
    }

    #[test]
    fn reader_is_header_driven_and_bom_tolerant() {
        let data = "\u{feff}STATE_PID|STATE_ABBREVIATION\n1|NSW\n";
        let mut reader =
            PsvReader::from_reader("state.psv".into(), std::io::Cursor::new(data)).unwrap();
        assert_eq!(reader.headers(), ["STATE_PID", "STATE_ABBREVIATION"]);
        assert_eq!(reader.required("STATE_PID").unwrap(), 0);
        assert!(reader.required("NOPE").is_err());
        let row = reader.next_row().unwrap().unwrap();
        assert_eq!(row, vec!["1", "NSW"]);
        assert!(reader.next_row().unwrap().is_none());
    }

    #[test]
    fn field_parsers_tolerate_empties() {
        assert_eq!(parse_u32(Some(&" 12 ".to_string())), Some(12));
        assert_eq!(parse_u32(Some(&"".to_string())), None);
        assert_eq!(parse_f64(Some(&"151.2".to_string())), Some(151.2));
        assert_eq!(parse_text(Some(&"  ".to_string())), None);
        assert_eq!(
            parse_text(Some(&" Sydney ".to_string())),
            Some("Sydney".into())
        );
    }
}
