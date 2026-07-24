//! Command-line surface — see `docs/PRD/02-cli-spec.md`.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use deepdoc_core::chunk::DEFAULT_CHUNK_SIZE;
use deepdoc_core::render::OutputFormat;

#[derive(Debug, Parser)]
#[command(
    name = "deepdoc",
    version,
    about = "Any document to clean Markdown, in one command.",
    long_about = "Extract clean Markdown, JSON or plain text from documents.\n\
                  Pure Rust, one static binary, no cloud, nothing to install."
)]
pub struct Args {
    /// Files to extract, or directories together with --recursive.
    #[arg(value_name = "INPUT", required = true)]
    pub inputs: Vec<PathBuf>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Md)]
    pub format: Format,

    /// Write to a file (single input) or a directory (batch); default is stdout.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Walk directories recursively, mirroring the tree into --output.
    #[arg(long)]
    pub recursive: bool,

    /// Page range for paginated formats: `1-10`, `3`, `2-`.
    #[arg(long, value_name = "RANGE")]
    pub pages: Option<String>,

    /// Prepend document metadata as YAML front-matter.
    #[arg(long)]
    pub metadata: bool,

    /// Split the output into RAG chunks of about SIZE characters.
    #[arg(
        long,
        value_name = "SIZE",
        num_args = 0..=1,
        default_missing_value = "800",
    )]
    pub chunk: Option<usize>,

    /// Overlap between neighbouring chunks, in characters.
    #[arg(long, value_name = "N", default_value_t = 0, requires = "chunk")]
    pub chunk_overlap: usize,

    /// Only report errors.
    #[arg(short, long, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Log more; repeat for more detail. Logs go to stderr.
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

/// `--format` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Md,
    Json,
    Text,
}

impl From<Format> for OutputFormat {
    fn from(format: Format) -> Self {
        match format {
            Format::Md => OutputFormat::Markdown,
            Format::Json => OutputFormat::Json,
            Format::Text => OutputFormat::Text,
        }
    }
}

impl Args {
    /// Chunk size requested with `--chunk`, if any.
    pub fn chunk_size(&self) -> Option<usize> {
        self.chunk
            .map(|size| if size == 0 { DEFAULT_CHUNK_SIZE } else { size })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_valid() {
        Args::command().debug_assert();
    }

    #[test]
    fn markdown_is_the_default_format() {
        let args = Args::try_parse_from(["deepdoc", "a.txt"]).unwrap();
        assert_eq!(args.format, Format::Md);
        assert!(args.chunk_size().is_none());
    }

    #[test]
    fn chunk_takes_an_optional_size() {
        let bare = Args::try_parse_from(["deepdoc", "a.txt", "--chunk"]).unwrap();
        assert_eq!(bare.chunk_size(), Some(DEFAULT_CHUNK_SIZE));

        let sized = Args::try_parse_from(["deepdoc", "a.txt", "--chunk", "400"]).unwrap();
        assert_eq!(sized.chunk_size(), Some(400));
    }

    #[test]
    fn an_input_is_required() {
        assert!(Args::try_parse_from(["deepdoc"]).is_err());
    }

    #[test]
    fn quiet_and_verbose_conflict() {
        assert!(Args::try_parse_from(["deepdoc", "a.txt", "-q", "-v"]).is_err());
    }
}
