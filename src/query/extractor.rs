use crate::query::Language;
use anyhow::{Context, Result};
use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use std::collections::HashSet;
use std::fmt::{self, Display};
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, Point, Query, QueryCursor};

/// Extractor for extracting syntax information of program
#[derive(Debug)]
pub struct Extractor {
    /// Language configuration
    language: Language,
    /// Language for tree_sitter
    ts_language: tree_sitter::Language,
    /// Tree_sitter query: a set of patterns that match nodes in a syntax tree.
    query: Query,
    /// Names of the captures used in the query.
    captures: Vec<String>,
    /// Ignored names with '_'
    ignores: HashSet<usize>,
}

impl Extractor {
    /// Build a new Extractor
    ///
    /// # Arguments
    ///
    /// * `language` - the language of source code
    ///
    /// * `query` - tree_sitter query
    ///
    /// # Returns
    ///
    /// * `Extractor` object
    ///
    /// # Example
    ///
    /// ```no_run
    /// # fn main() -> anyhow::Result<()> {
    /// use rust_hero::query::{Language,Extractor};
    ///
    /// let lang = Language::Rust;
    /// let query = lang
    ///     .parse_query("(import_clause (upper_case_qid)@import)")
    ///     .unwrap();
    /// let extractor = Extractor::new(lang, query);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(language: Language, query: Query) -> Extractor {
        let captures = query.capture_names().to_vec();

        let mut ignores = HashSet::default();
        captures.iter().enumerate().for_each(|(i, name)| {
            if name.starts_with('_') {
                ignores.insert(i);
            }
        });

        Extractor {
            ts_language: (&language).language(),
            language,
            query,
            captures,
            ignores,
        }
    }

    /// Get the language of Extractor
    pub fn language(&self) -> &Language {
        &self.language
    }

    /// Extracted query information from one source file
    pub fn extract_from_file(
        &self,
        path: &Path,
        parser: &mut Parser,
    ) -> Result<Option<ExtractedFile>> {
        let source = fs::read(&path).context("could not read file")?;

        self.extract_from_text(Some(path), &source, parser)
    }

    /// Extracted query information from one fragment program
    ///     
    /// # Arguments
    ///
    /// * `path` - Option: the path of source file
    ///
    /// * `source` - fragment program
    ///
    /// * `parser` - tree_sitter Parser
    ///
    /// # Returns
    ///
    /// * `ExtractedFile` object
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> anyhow::Result<()> {
    /// use rust_hero::query::{Language,Extractor};
    /// use tree_sitter::Parser;
    ///
    /// let lang = Language::Rust;
    /// let query = lang
    ///     .parse_query("(function_item (identifier) @id) @function")
    ///     .unwrap();
    /// let extractor = Extractor::new(lang, query);
    ///         let extracted = extractor
    ///        .extract_from_text(None, b"fn main(){println!(\"hello rust_hero\");}", &mut Parser::new())
    ///        // From Result<Option<ExtractedFile>>
    ///        .unwrap()
    ///        // From Option<ExtractedFile>
    ///        .unwrap();
    ///
    /// println!("{:?},{:?}，{:?}",extracted.matches.len(),extracted.matches[0].name,extracted.matches[0].text);
    /// assert_eq!(extracted.matches.len(), 2);
    /// assert_eq!(extracted.matches[0].name, "function");
    /// assert_eq!(extracted.matches[0].text, "fn main(){println!(\"hello rust_hero\");}");
    /// # Ok(())
    /// # }
    /// ```
    pub fn extract_from_text(
        &self,
        path: Option<&Path>,
        source: &[u8],
        parser: &mut Parser,
    ) -> Result<Option<ExtractedFile>> {
        parser
            .set_language(self.ts_language)
            .context("could not set language")?;

        let tree = parser
            .parse(&source, None)
            // note: this could be a timeout or cancellation, but we don't set
            // that so we know it's always a language error. Buuuut we also
            // always set the language above so if this happens we also know
            // it's an internal error.
            .context(
                "could not parse to a tree. This is an internal error and should be reported.",
            )?;

        let mut cursor = QueryCursor::new();

        let extracted_matches = cursor
            .matches(&self.query, tree.root_node(), source)
            .flat_map(|query_match| query_match.captures)
            // note: the casts here could potentially break if run on a 16-bit
            // microcontroller. I don't think this is a huge problem, though,
            // since even the gnarliest queries I've written have something on
            // the order of 20 matches. Nowhere close to 2^16!
            .filter(|capture| !self.ignores.contains(&(capture.index as usize)))
            .map(|capture| {
                let name = &self.captures[capture.index as usize];
                let node = capture.node;
                let text = match node
                    .utf8_text(source)
                    .map(|unowned| unowned.to_string())
                    .context("could not extract text from capture")
                {
                    Ok(text) => text,
                    Err(problem) => return Err(problem),
                };

                Ok(ExtractedMatch {
                    kind: node.kind(),
                    name,
                    text,
                    start: node.start_position(),
                    end: node.end_position(),
                })
            })
            .collect::<Result<Vec<ExtractedMatch>>>()?;

        if extracted_matches.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ExtractedFile {
                file: path.map(|p| p.to_owned()),
                file_type: self.language.to_string(),
                matches: extracted_matches,
            }))
        }
    }
}

/// Extracted query from source file
#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtractedFile<'query> {
    /// Extracted source file
    pub file: Option<PathBuf>,
    /// Language
    pub file_type: String,
    /// A set of patterns that match nodes in a syntax tree.
    pub matches: Vec<ExtractedMatch<'query>>,
}

impl<'query> Display for ExtractedFile<'query> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // TODO: is there a better way to do this unwrapping? This implementation
        // turns non-UTF-8 paths into "NON-UTF8 FILENAME". I don't know exactly
        // what circumstances that could happen in... maybe we should just wait
        // for bug reports?
        let filename = self
            .file
            .as_ref()
            .map(|f| f.to_str().unwrap_or("NON-UTF8 FILENAME"))
            .unwrap_or("NO FILE");

        for extraction in &self.matches {
            writeln!(
                f,
                "{}:{}:{}:{}:{}",
                filename,
                extraction.start.row + 1,
                extraction.start.column + 1,
                extraction.name,
                extraction.text
            )?
        }

        Ok(())
    }
}

/// Pattern matching nodes in a syntax tree.
#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtractedMatch<'query> {
    /// Pattern type
    kind: &'static str,
    /// Pattern name
    pub name: &'query str,
    /// Fragment program
    pub text: String,
    /// Start cordinate of current text
    #[serde(serialize_with = "serialize_point")]
    pub start: Point,
    /// End cordinate of current text
    #[serde(serialize_with = "serialize_point")]
    pub end: Point,
}

fn serialize_point<S>(point: &Point, sz: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut out = sz.serialize_struct("Point", 2)?;
    out.serialize_field("row", &(point.row + 1))?;
    out.serialize_field("column", &(point.column + 1))?;
    out.end()
}
