//! Routing: inputs → `detect` → `extract` → `render` → stdout or files.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use deepdoc_core::chunk::{Chunk, ChunkOpts, chunk};
use deepdoc_core::extract::{ExtractOpts, PageRange, extract_path};
use deepdoc_core::render::{OutputFormat, RenderOpts, render};
use deepdoc_core::{Document, exit_code};
use rayon::prelude::*;

use crate::args::{Args, Format};
use crate::log::{Level, Logger};

/// One input file and where its output goes, relative to `-o` (batch mode).
struct Job {
    path: PathBuf,
    relative: PathBuf,
    /// Output path under `-o`, with the extension of the chosen format.
    target: PathBuf,
    /// True when a directory walk found this file rather than the command line
    /// naming it. A walked folder holds whatever it holds, so "unsupported" or
    /// "no text" is a skip there; for a file the user named, it is a failure.
    discovered: bool,
}

/// A problem found while listing the inputs, before anything was extracted.
struct Problem {
    error: anyhow::Error,
    /// Skips are reported and counted; failures also set the exit code.
    skip: bool,
}

/// What became of one job.
enum Outcome {
    /// Written to this path.
    Written(PathBuf),
    /// Rendered, still waiting to go to stdout in input order.
    Printed(String),
    Skipped(anyhow::Error),
    Failed(anyhow::Error),
}

/// Everything the per-file work needs; shared across the worker threads.
struct Run<'a> {
    args: &'a Args,
    extract_opts: ExtractOpts,
    render_opts: RenderOpts,
    format: OutputFormat,
    output_is_dir: bool,
}

/// Run the whole invocation and return the process exit code.
pub fn run(args: &Args) -> Result<i32> {
    let logger = Logger::new(Level::from_flags(args.quiet, args.verbose));

    let extract_opts = ExtractOpts {
        pages: args.pages.as_deref().map(PageRange::parse).transpose()?,
    };

    let (mut jobs, problems) = collect_jobs(args);
    assign_targets(&mut jobs, output_extension(args));
    if jobs.is_empty() && problems.is_empty() {
        logger.warn("nothing to extract");
        return Ok(exit_code::OK);
    }

    // `-o` is a directory whenever more than one document can come out of the run.
    let output_is_dir = args.recursive || jobs.len() > 1;
    if output_is_dir && args.output.is_none() {
        logger.verbose("writing every document to stdout; use -o <dir> to split them into files");
    }

    let context = Run {
        args,
        extract_opts,
        render_opts: RenderOpts {
            metadata: args.metadata,
        },
        format: args.format.into(),
        output_is_dir,
    };

    // Files are extracted in parallel but reported in input order: a batch that
    // shuffled its stdout or its log from run to run would not be the
    // deterministic tool the spec promises.
    let outcomes: Vec<Outcome> = jobs.par_iter().map(|job| process(job, &context)).collect();

    let mut extracted = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut first_failure = None;
    let mut first_skip = None;

    for problem in &problems {
        if problem.skip {
            skipped += 1;
            first_skip.get_or_insert(exit_code_for(&problem.error));
            logger.warn(format!("skipped {:#}", problem.error));
        } else {
            failed += 1;
            first_failure.get_or_insert(exit_code_for(&problem.error));
            logger.error(format!("{:#}", problem.error));
        }
    }

    for (job, outcome) in jobs.iter().zip(outcomes) {
        match outcome {
            Outcome::Written(target) => {
                extracted += 1;
                logger.info(format!("{} → {}", job.path.display(), target.display()));
            }
            Outcome::Printed(text) => {
                extracted += 1;
                let mut stdout = std::io::stdout().lock();
                stdout
                    .write_all(text.as_bytes())
                    .context("cannot write to stdout")?;
            }
            Outcome::Skipped(error) => {
                skipped += 1;
                first_skip.get_or_insert(exit_code_for(&error));
                logger.warn(format!("skipped {error:#}"));
            }
            Outcome::Failed(error) => {
                failed += 1;
                first_failure.get_or_insert(exit_code_for(&error));
                logger.error(format!("{error:#}"));
            }
        }
    }

    if jobs.len() + problems.len() > 1 {
        logger.info(summary(extracted, skipped, failed));
    }

    // A batch survives its skips, but a run that produced nothing should not
    // claim success — then the first skip decides the code (4 scan / 5 type).
    let code = first_failure
        .or_else(|| (extracted == 0).then_some(first_skip).flatten())
        .unwrap_or(exit_code::OK);
    Ok(code)
}

/// Extract, render and deliver one file.
fn process(job: &Job, context: &Run) -> Outcome {
    match extract_one(job, context).and_then(|rendered| write_output(job, &rendered, context)) {
        Ok(outcome) => outcome,
        Err(error) if job.discovered && is_skip(&error) => Outcome::Skipped(error),
        Err(error) => Outcome::Failed(error),
    }
}

/// Extract a single file and render it.
fn extract_one(job: &Job, context: &Run) -> Result<String> {
    let args = context.args;
    let doc: Document = extract_path(&job.path, &context.extract_opts)?;

    if let Some(size) = args.chunk_size() {
        let opts = ChunkOpts {
            size,
            overlap: args.chunk_overlap,
        };
        return Ok(render_chunks(
            &chunk(&doc, &opts),
            &doc,
            &job.path,
            args.format,
        ));
    }

    let mut rendered = render(&doc, context.format, &context.render_opts);
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

/// Serialize chunks in the requested output format.
///
/// JSON is the shape integrations consume (`docs/PRD/02-cli-spec.md`); the
/// Markdown and text forms print the same chunks with a separator, so a human
/// can see where the splitter cut before feeding a pipeline.
fn render_chunks(chunks: &[Chunk], doc: &Document, path: &Path, format: Format) -> String {
    if let Format::Json = format {
        let value = serde_json::json!({
            "source": path.display().to_string(),
            "meta": doc.meta,
            "chunks": chunks,
        });
        return format!("{value}\n");
    }

    let mut out = String::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let (start, end) = chunk.byte_range;
        let path = chunk.heading_path.join(" > ");
        let label = format!(
            "chunk {}/{} | {} | bytes {start}-{end}",
            index + 1,
            chunks.len(),
            if path.is_empty() { "—" } else { &path },
        );
        out.push_str(&match format {
            Format::Md => format!("<!-- {label} -->\n\n"),
            _ => format!("-- {label} --\n\n"),
        });
        out.push_str(chunk.text.trim_end());
        out.push_str("\n\n");
    }
    out
}

/// Whether an error means "not something to extract here" rather than a failure.
fn is_skip(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<deepdoc_core::Error>(),
        Some(
            deepdoc_core::Error::Unsupported { .. }
                | deepdoc_core::Error::NotImplemented { .. }
                | deepdoc_core::Error::NoText { .. }
        )
    )
}

/// The end-of-run tally from the spec.
fn summary(extracted: usize, skipped: usize, failed: usize) -> String {
    let mut line = format!("{extracted} extracted, {skipped} skipped");
    if failed > 0 {
        line.push_str(&format!(", {failed} failed"));
    }
    // The tick claims a good run; a batch that produced nothing has not had one.
    if failed == 0 && extracted > 0 {
        line.insert_str(0, "✓ ");
    }
    line
}

/// Expand the command line into the list of files to process.
///
/// Listing problems are collected rather than raised: one unreadable folder in
/// a tree must not cost the user every other document in it.
fn collect_jobs(args: &Args) -> (Vec<Job>, Vec<Problem>) {
    let mut jobs = Vec::new();
    let mut problems = Vec::new();

    for input in &args.inputs {
        let meta = match std::fs::metadata(input) {
            Ok(meta) => meta,
            Err(error) => {
                problems.push(Problem {
                    error: anyhow!("cannot read {}: {error}", input.display()),
                    skip: false,
                });
                continue;
            }
        };

        if meta.is_dir() {
            if args.recursive {
                collect_dir(input, input, &mut jobs, &mut problems);
            } else {
                problems.push(Problem {
                    error: anyhow!(
                        "{} is a directory — pass --recursive to walk it",
                        input.display()
                    ),
                    skip: false,
                });
            }
        } else {
            let relative = input
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| input.clone());
            jobs.push(Job {
                path: input.clone(),
                relative,
                target: PathBuf::new(),
                discovered: false,
            });
        }
    }

    // Walk order is whatever the filesystem hands back; sorting here is what
    // makes two runs over the same tree print the same thing.
    jobs.sort_by(|a, b| a.path.cmp(&b.path));
    (jobs, problems)
}

/// Walk `dir`, recording every file relative to `root`.
fn collect_dir(root: &Path, dir: &Path, jobs: &mut Vec<Job>, problems: &mut Vec<Problem>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            problems.push(Problem {
                error: anyhow!("cannot read {}: {error}", dir.display()),
                skip: true,
            });
            return;
        }
    };

    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            collect_dir(root, &path, jobs, problems);
        } else if file_type.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            jobs.push(Job {
                path,
                relative,
                target: PathBuf::new(),
                discovered: true,
            });
        }
    }
}

/// Decide where each job's output goes.
///
/// `report.pdf` becomes `report.md`, but a folder holding `report.pdf` and
/// `report.docx` would then write both to the same file. Colliding names keep
/// their original extension (`report.pdf.md`) so a batch never silently drops a
/// document. The decision looks at the whole job list, so it does not depend on
/// the order files were walked in.
fn assign_targets(jobs: &mut [Job], extension: &str) {
    let mut counts: HashMap<PathBuf, usize> = HashMap::new();
    for job in jobs.iter() {
        *counts
            .entry(job.relative.with_extension(extension))
            .or_default() += 1;
    }

    for job in jobs.iter_mut() {
        let short = job.relative.with_extension(extension);
        job.target = if counts.get(&short).copied().unwrap_or(0) > 1 {
            let mut name = job.relative.file_name().unwrap_or_default().to_os_string();
            name.push(".");
            name.push(extension);
            job.relative.with_file_name(name)
        } else {
            short
        };
    }
}

/// Write a rendered document, or hand it back for stdout.
fn write_output(job: &Job, rendered: &str, context: &Run) -> Result<Outcome> {
    let Some(output) = context.args.output.as_ref() else {
        return Ok(Outcome::Printed(rendered.to_string()));
    };

    let target = if context.output_is_dir {
        output.join(&job.target)
    } else {
        output.clone()
    };

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    std::fs::write(&target, rendered)
        .with_context(|| format!("cannot write {}", target.display()))?;

    Ok(Outcome::Written(target))
}

/// File extension for the chosen output format.
fn output_extension(args: &Args) -> &'static str {
    match args.format {
        Format::Md => "md",
        Format::Json => "json",
        Format::Text => "txt",
    }
}

/// Map an error onto the exit code from the spec.
pub fn exit_code_for(error: &anyhow::Error) -> i32 {
    error
        .downcast_ref::<deepdoc_core::Error>()
        .map(deepdoc_core::Error::exit_code)
        .unwrap_or(exit_code::FAILURE)
}
