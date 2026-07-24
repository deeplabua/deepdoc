//! Routing: inputs → `detect` → `extract` → `render` → stdout or files.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use deepdoc_core::chunk::{ChunkOpts, chunk};
use deepdoc_core::extract::{ExtractOpts, PageRange, extract_path};
use deepdoc_core::render::{OutputFormat, RenderOpts, render};
use deepdoc_core::{Document, exit_code};

use crate::args::Args;
use crate::log::{Level, Logger};

/// One input file and where its output goes, relative to `-o` (batch mode).
struct Job {
    path: PathBuf,
    relative: PathBuf,
    /// Output path under `-o`, with the extension of the chosen format.
    target: PathBuf,
}

/// Run the whole invocation and return the process exit code.
pub fn run(args: &Args) -> Result<i32> {
    let logger = Logger::new(Level::from_flags(args.quiet, args.verbose));

    let extract_opts = ExtractOpts {
        pages: args.pages.as_deref().map(PageRange::parse).transpose()?,
    };
    let render_opts = RenderOpts {
        metadata: args.metadata,
    };
    let format: OutputFormat = args.format.into();

    let mut jobs = collect_jobs(args)?;
    assign_targets(&mut jobs, output_extension(args));
    if jobs.is_empty() {
        logger.warn("nothing to extract");
        return Ok(exit_code::OK);
    }

    // `-o` is a directory whenever more than one document can come out of the run.
    let output_is_dir = args.recursive || jobs.len() > 1;
    if output_is_dir && args.output.is_none() {
        logger.verbose("writing every document to stdout; use -o <dir> to split them into files");
    }

    let mut failures = 0usize;
    let mut first_failure_code = None;

    for job in &jobs {
        match extract_one(job, &extract_opts, format, &render_opts, args) {
            Ok(rendered) => {
                write_output(job, &rendered, args, output_is_dir, &logger)?;
            }
            Err(error) => {
                failures += 1;
                let code = exit_code_for(&error);
                first_failure_code.get_or_insert(code);
                logger.error(format!("{}: {error:#}", job.path.display()));
            }
        }
    }

    let extracted = jobs.len() - failures;
    if jobs.len() > 1 {
        logger.info(format!("{extracted} extracted, {failures} failed"));
    }

    Ok(first_failure_code.unwrap_or(exit_code::OK))
}

/// Extract a single file and render it.
fn extract_one(
    job: &Job,
    extract_opts: &ExtractOpts,
    format: OutputFormat,
    render_opts: &RenderOpts,
    args: &Args,
) -> Result<String> {
    let doc: Document = extract_path(&job.path, extract_opts)?;

    if let Some(size) = args.chunk_size() {
        let opts = ChunkOpts {
            size,
            overlap: args.chunk_overlap,
        };
        let chunks = chunk(&doc, &opts);
        // TODO(Phase 6): chunks in Markdown/text output too, not just JSON.
        let value = serde_json::json!({
            "source": job.path.display().to_string(),
            "metadata": doc.meta,
            "chunks": chunks,
        });
        return Ok(format!("{value}\n"));
    }

    let mut rendered = render(&doc, format, render_opts);
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

/// Expand the command line into the list of files to process.
fn collect_jobs(args: &Args) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();

    for input in &args.inputs {
        let meta =
            std::fs::metadata(input).with_context(|| format!("cannot read {}", input.display()))?;

        if meta.is_dir() {
            if !args.recursive {
                bail!(
                    "{} is a directory — pass --recursive to walk it",
                    input.display()
                );
            }
            collect_dir(input, input, &mut jobs)?;
        } else {
            let relative = input
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| input.clone());
            jobs.push(Job {
                path: input.clone(),
                relative,
                target: PathBuf::new(),
            });
        }
    }

    jobs.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(jobs)
}

/// Walk `dir`, recording every file relative to `root`.
// TODO(Phase 6): parallel batch (rayon) and the per-file progress report from the spec.
fn collect_dir(root: &Path, dir: &Path, jobs: &mut Vec<Job>) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?;

    for entry in entries {
        let entry = entry.with_context(|| format!("cannot read {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            collect_dir(root, &path, jobs)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            jobs.push(Job {
                path,
                relative,
                target: PathBuf::new(),
            });
        }
    }
    Ok(())
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

/// Write a rendered document to stdout or to the output path.
fn write_output(
    job: &Job,
    rendered: &str,
    args: &Args,
    output_is_dir: bool,
    logger: &Logger,
) -> Result<()> {
    let Some(output) = args.output.as_ref() else {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(rendered.as_bytes())
            .context("cannot write to stdout")?;
        return Ok(());
    };

    let target = if output_is_dir {
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

    logger.info(format!("{} → {}", job.path.display(), target.display()));
    Ok(())
}

/// File extension for the chosen output format.
fn output_extension(args: &Args) -> &'static str {
    use crate::args::Format;
    match args.format {
        _ if args.chunk_size().is_some() => "json",
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
