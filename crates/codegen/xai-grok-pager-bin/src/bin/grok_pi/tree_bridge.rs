use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Inject a headless Pi extension that exposes tree control over RPC without
/// modifying Pi source. Hidden from slash UI by adapter filtering.
pub(super) fn write_navigate_tree_extension() -> Result<NamedTempFile> {
    // NamedTempFile defaults to no suffix; force `.ts` so Pi's loader accepts it.
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-tree-bridge-")
        .suffix(".ts")
        .tempfile()
        .context("create tree bridge extension tempfile")?;
    // Official ExtensionCommandContext: navigateTree + setLabel (rpc-mode).
    const SOURCE: &str = r#"export default function (pi) {
  pi.registerCommand("__pi_navigate_tree", {
    description: "Internal Pi-Grok bridge: navigate session tree leaf",
    handler: async (args, ctx) => {
      const raw = String(args ?? "").trim();
      if (!raw) throw new Error("entry id required");
      const summarize = /(?:^|\s)--summarize(?:\s|$)/.test(raw);
      let customInstructions;
      const instrMatch = raw.match(/(?:^|\s)--instructions\s+([\s\S]+)$/);
      if (instrMatch) customInstructions = instrMatch[1].trim();
      const entryId = raw
        .replace(/(?:^|\s)--summarize(?:\s|$)/g, " ")
        .replace(/(?:^|\s)--instructions\s+[\s\S]+$/, " ")
        .trim()
        .split(/\s+/)[0];
      if (!entryId) throw new Error("entry id required");
      const result = await ctx.navigateTree(entryId, {
        summarize,
        customInstructions: customInstructions || undefined,
      });
      if (result?.cancelled) throw new Error("tree navigation cancelled");
    },
  });

  pi.registerCommand("__pi_tree_label", {
    description: "Internal Pi-Grok bridge: set/clear session tree label",
    handler: async (args, ctx) => {
      const raw = String(args ?? "").trim();
      if (!raw) throw new Error("entry id required");
      const tokens = raw.split(/\s+/);
      const entryId = tokens[0];
      if (!entryId) throw new Error("entry id required");
      if (tokens.includes("--clear")) {
        ctx.setLabel(entryId, undefined);
        return;
      }
      const label = raw.slice(entryId.length).trim();
      ctx.setLabel(entryId, label || undefined);
    },
  });
}
"#;
    file.write_all(SOURCE.as_bytes())
        .context("write tree bridge extension source")?;
    file.flush().context("flush tree bridge extension")?;
    // Ensure the file is durable before Pi spawns.
    File::open(file.path()).and_then(|f| f.sync_all()).ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigate_tree_extension_source_is_valid_ts_module() {
        let file = write_navigate_tree_extension().expect("temp extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("registerCommand(\"__pi_navigate_tree\""));
        assert!(source.contains("registerCommand(\"__pi_tree_label\""));
        assert!(source.contains("ctx.navigateTree"));
        assert!(source.contains("ctx.setLabel"));
        assert!(file.path().extension().and_then(|e| e.to_str()) == Some("ts"));
    }
}
