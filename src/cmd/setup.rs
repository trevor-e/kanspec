//! `setup claude|cursor|codex [--remove]`, `instructions [topic]`, `completions <shell>`.
//!
//! Owner: **S7**.

use serde::Serialize;

use crate::cli::{CompletionsArgs, InstructionsArgs, SetupArgs};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::instructions::Topic;
use crate::out::{glyph, Line, Render, Style};

#[derive(Debug, Serialize)]
pub struct SetupReport {
    pub agent: &'static str,
    pub removed: bool,
    pub changes: Vec<crate::setup::SetupChange>,
    pub next: Vec<String>,
}

pub fn setup(ctx: &Ctx, a: &SetupArgs) -> Result<SetupReport> {
    let r = if a.remove {
        crate::setup::remove(ctx, a.agent)?
    } else {
        // Installing the snippet into a repo with no store would point an agent at verbs
        // that all refuse. `--remove` deliberately does not require one: cleaning up after
        // a deleted store must always work.
        ctx.require_initialized()?;
        crate::setup::install(ctx, a.agent)?
    };
    Ok(SetupReport {
        agent: r.agent,
        removed: a.remove,
        changes: r.files,
        next: if a.remove {
            vec![
                format!("{} setup {}", ctx.invoked_as, r.agent),
                // The git hooks went with it; this is how they come back without
                // re-scaffolding.
                format!("{} init --refresh-hooks", ctx.invoked_as),
            ]
        } else {
            vec![
                format!("{} prime", ctx.invoked_as),
                format!("{} instructions", ctx.invoked_as),
            ]
        },
    })
}

impl Render for SetupReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        let verb = if self.removed { "removed" } else { "installed" };
        writeln!(w, " {} {verb} for {}", glyph::OK, self.agent)?;
        for c in &self.changes {
            let g = if c.changed { glyph::OK } else { '·' };
            let note = if c.changed {
                c.what.to_string()
            } else {
                format!("{} · unchanged", c.what)
            };
            Line::new(g, c.path.display().to_string())
                .dim(note)
                .write(w, st)?;
        }
        for n in &self.next {
            writeln!(w, " {} {n}", glyph::FIX)?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct InstructionsReport {
    /// present when a topic was named
    pub topic: Option<String>,
    pub text: Option<String>,
    /// the topic list, printed when no topic was named
    pub topics: Vec<Topic>,
}

pub fn instructions(_ctx: &Ctx, a: &InstructionsArgs) -> Result<InstructionsReport> {
    match &a.topic {
        Some(t) => Ok(InstructionsReport {
            text: Some(crate::instructions::render(t)?),
            topic: Some(t.clone()),
            topics: crate::instructions::topics(),
        }),
        None => Ok(InstructionsReport {
            topic: None,
            text: None,
            topics: crate::instructions::topics(),
        }),
    }
}

impl Render for InstructionsReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // A named topic prints its markdown and nothing else: this output is read by a
        // human in a pager and by an agent through a pipe, and neither wants a banner.
        if let Some(text) = &self.text {
            return write!(w, "{text}");
        }
        writeln!(w, " workflow docs, versioned with the binary")?;
        for t in &self.topics {
            Line::new('·', format!("{:<10} {}", t.name, t.title)).write(w, st)?;
        }
        writeln!(
            w,
            " {} {} instructions <topic>",
            glyph::FIX,
            crate::cli::invoked_as()
        )
    }
}

#[derive(Debug, Serialize)]
pub struct CompletionsReport {
    pub shell: String,
    pub script: String,
}

pub fn completions(ctx: &Ctx, a: &CompletionsArgs) -> Result<CompletionsReport> {
    use clap::CommandFactory;
    let mut cmd = crate::cli::Cli::command()
        .name(ctx.invoked_as)
        .bin_name(ctx.invoked_as);
    let mut buf: Vec<u8> = Vec::new();
    clap_complete::generate(a.shell, &mut cmd, ctx.invoked_as, &mut buf);
    Ok(CompletionsReport {
        shell: a.shell.to_string(),
        script: String::from_utf8_lossy(&buf).into_owned(),
    })
}

impl Render for CompletionsReport {
    fn human(&self, w: &mut dyn std::io::Write, _st: &Style) -> std::io::Result<()> {
        // Piped straight into the shell — nothing else may be printed, not even a newline
        // clap did not generate.
        write!(w, "{}", self.script)
    }
}
