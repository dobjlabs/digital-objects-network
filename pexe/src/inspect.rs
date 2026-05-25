//! `pexe inspect` subcommand handlers — read-only views of a plugin's
//! predicates, classes, and action graph. Each handler accepts a target
//! path that is either a `.pexe` archive or a source directory holding
//! `manifest.toml` + `plugin.rhai`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result, anyhow};
use pod2::lang::PrettyPrint;
use pod2::middleware::{
    CustomPredicateBatch, Hash, NativePredicate, Predicate, PredicateOrWildcard, StatementTmpl,
    StatementTmplArg, Wildcard,
};
use sdk::{Dependency, Sdk, SdkModule, manifest::Manifest};

use crate::{PluginSource, unpack};

/// Hash of `Predicate::Custom(txlib::TxInsert)`. Computed once on first
/// access; identifies txlib's TxInsert event regardless of which batch
/// referenced it.
static TX_INSERT_HASH: LazyLock<Hash> = LazyLock::new(|| txlib_event_hash("TxInsert"));
static TX_MUTATE_HASH: LazyLock<Hash> = LazyLock::new(|| txlib_event_hash("TxMutate"));

fn txlib_event_hash(name: &str) -> Hash {
    let module = txlib::predicates::module();
    let custom_ref = module
        .batch
        .predicate_ref_by_name(name)
        .unwrap_or_else(|| panic!("txlib module is missing predicate {name}"));
    Predicate::Custom(custom_ref).hash()
}

/// Resolve a target path to its parsed manifest and plugin script.
/// Directories are read via `PluginSource::read`; anything else is
/// treated as a `.pexe` archive and unpacked.
fn load_target(path: &Path) -> Result<(Manifest, String)> {
    if path.is_dir() {
        let source = PluginSource::read(path)?;
        let manifest = source.parse_manifest()?;
        Ok((manifest, source.script))
    } else {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        unpack(&bytes)
    }
}

/// Compile the plugin script with the manifest's action list and return
/// the loaded SDK module.
fn load_sdk_module(manifest: &Manifest, script: &str) -> Result<std::rc::Rc<SdkModule>> {
    let sdk = Sdk::default();
    let action_names: Vec<&str> = manifest.actions.iter().map(|a| a.name.as_str()).collect();
    sdk.load_module_from_src_actions(script, &action_names)
        .map_err(|err| anyhow!("failed to compile plugin: {err}"))
}

/// `pexe inspect predicates`.
///
/// Without `--middleware`, prints the SDK-synthesized Podlang source
/// (the form authored by the SDK frontend before pod2's compiler runs).
/// With `--middleware`, walks the compiled [`pod2::middleware::CustomPredicateBatch`]
/// and renders each predicate via [`PrettyPrint::to_podlang_string`].
///
/// When `action` is `Some`, filters output to predicates whose name
/// matches exactly. When `None`, emits everything.
pub fn predicates(target: &Path, action: Option<&str>, middleware: bool) -> Result<()> {
    let (manifest, script) = load_target(target)?;
    let module = load_sdk_module(&manifest, &script)?;

    if middleware {
        print_middleware(&module, action)
    } else {
        print_frontend(&module, action)
    }
}

fn print_middleware(module: &SdkModule, action: Option<&str>) -> Result<()> {
    let batch = &module.module().batch;
    let predicates = batch.predicates();
    // Render the whole batch once with batch context so each
    // `BatchSelf(N)` reference inside a statement gets resolved to the
    // target predicate's name. We then pick out individual blocks by
    // name to avoid emitting predicates the user didn't ask for.
    let batch_text = batch.to_podlang_string();
    if let Some(name) = action {
        // Filtering to a single predicate also pulls in its split
        // chain: the transitive closure of helper predicates reachable
        // through `BatchSelf(N)` references. Stop at top-level actions
        // (subaction calls); those are their own root and the user can
        // request them separately.
        let action_names: BTreeSet<&str> =
            module.actions().iter().map(|a| a.name.as_str()).collect();
        let start = predicates
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| anyhow!("no predicate named {name} in this plugin"))?;
        let mut order: Vec<usize> = Vec::new();
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        let mut queue: Vec<usize> = vec![start];
        while let Some(idx) = queue.pop() {
            if !seen.insert(idx) {
                continue;
            }
            order.push(idx);
            let Some(pred) = predicates.get(idx) else {
                continue;
            };
            for stmt in pred.statements() {
                if let PredicateOrWildcard::Predicate(Predicate::BatchSelf(child)) =
                    &stmt.pred_or_wc
                {
                    let Some(child_pred) = predicates.get(*child) else {
                        continue;
                    };
                    if action_names.contains(child_pred.name.as_str()) && *child != start {
                        continue;
                    }
                    queue.push(*child);
                }
            }
        }
        let mut first = true;
        for idx in order {
            let Some(pred) = predicates.get(idx) else {
                continue;
            };
            let Some(block) = find_predicate_block(&batch_text, &pred.name) else {
                continue;
            };
            if !first {
                println!();
            }
            first = false;
            println!("{}", block);
        }
        return Ok(());
    }
    print!("{}", batch_text);
    if !batch_text.ends_with('\n') {
        println!();
    }
    Ok(())
}

fn print_frontend(module: &SdkModule, action: Option<&str>) -> Result<()> {
    let src = module.podlang_src();
    match action {
        None => {
            print!("{}", src);
            if !src.ends_with('\n') {
                println!();
            }
            Ok(())
        }
        Some(name) => {
            let block = find_predicate_block(src, name)
                .ok_or_else(|| anyhow!("no predicate named {name} in synthesized Podlang"))?;
            // Records referenced by typed wildcards in the predicate's
            // signature: emit each definition before the predicate so
            // the output is self-contained.
            let records = referenced_records(block);
            for record_name in &records {
                if let Some(decl) = find_record_decl(src, record_name) {
                    println!("{}", decl);
                }
            }
            if !records.is_empty() {
                println!();
            }
            println!("{}", block);
            Ok(())
        }
    }
}

/// Type names appearing as the second token of any arg in a predicate
/// signature. Looks at the text between the opening `(` and matching
/// `)`, splits by `,`, and for each part returns the second
/// whitespace-separated word if there is one. Scans both public and
/// private arg sections; chain records (e.g. `<Action>Chain`) live
/// after `private:` and the user wants them surfaced too.
fn referenced_records(block: &str) -> Vec<String> {
    let Some(open) = block.find('(') else {
        return Vec::new();
    };
    let mut depth = 0usize;
    let mut close: Option<usize> = None;
    for (i, c) in block[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return Vec::new();
    };
    let inside = &block[open + 1..close];
    let mut out: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for arg in inside.split(',') {
        // Strip the `private:` marker so the first arg in the private
        // section parses like any other (`private: chain_steps Chain`).
        let cleaned = arg.replace("private:", " ");
        let tokens: Vec<&str> = cleaned.split_whitespace().collect();
        if tokens.len() < 2 {
            continue;
        }
        // Capitalized second token = a record name. Lowercase second
        // tokens would indicate something we don't currently model.
        let type_name = tokens[1].trim_end_matches(':');
        if !type_name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
        {
            continue;
        }
        let type_name = type_name.to_string();
        if seen.insert(type_name.clone()) {
            out.push(type_name);
        }
    }
    out
}

/// Find the line `record <Name> = (...)` in the source, if present.
fn find_record_decl<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("record {name} = ");
    for line in src.lines() {
        if line.starts_with(&needle) {
            return Some(line);
        }
    }
    None
}

/// `pexe inspect plan`.
///
/// Mint one synthetic object per input declared by `action`, fabricate
/// a grounded state for them, and run the SDK's solver in mock mode
/// without proving. Prints three sections to stdout:
///
/// 1. **Header** — action name with input / output classes.
/// 2. **Solution breakdown** — the multi-pod solver's per-POD
///    utilization summary (statements, merkle proofs, etc.).
/// 3. **Statement dep graph** — per-POD list of statements in chain
///    order, labelled by predicate name with internal dependency
///    indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlanSection {
    Header,
    Summary,
    Totals,
    Deps,
}

impl PlanSection {
    /// All sections that `--show all` (or no `--show` flag) expand to.
    pub fn default_all() -> [PlanSection; 4] {
        [Self::Header, Self::Summary, Self::Totals, Self::Deps]
    }
}

#[derive(Clone, Debug)]
pub enum PlanOutput {
    Text(BTreeSet<PlanSection>),
    DotCompressed,
    DotFull,
    MermaidCompressed,
    MermaidFull,
    MermaidLinkCompressed,
    MermaidLinkFull,
}

/// `pexe prove`. Same shape as `inspect plan` but with `mock=false`,
/// so the action is actually proved via the real plonky2 prover. This
/// is much slower than `plan` (minutes for actions with many PODs).
pub fn prove_action(target: &Path, action_name: &str) -> Result<()> {
    let (manifest, script) = load_target(target)?;
    let module = load_sdk_module(&manifest, &script)?;
    let action = module
        .actions()
        .iter()
        .find(|a| a.name == action_name)
        .ok_or_else(|| anyhow!("no action named {action_name} in this plugin"))?;

    let input_classes: Vec<String> = action.total_inputs().map(|r| r.class.clone()).collect();
    let output_classes: Vec<String> = action.total_outputs().map(|r| r.class.clone()).collect();

    println!("Prove: {}", action_name);
    println!("  Inputs ({}):", input_classes.len());
    for class in &input_classes {
        println!("    - {class}");
    }
    println!("  Outputs ({}):", output_classes.len());
    for class in &output_classes {
        println!("    - {class}");
    }
    println!();
    println!("Proving via real plonky2 backend — this may take several minutes.");
    println!();

    let mut minted = Vec::with_capacity(input_classes.len());
    for class in &input_classes {
        let obj = crate::fixtures::mint_class(&module, class)?;
        minted.push(obj);
    }
    let state = crate::fixtures::build_synthetic_state(&minted)?;
    let executor = module.executor(false, state.grounding_witness.clone());

    let start = std::time::Instant::now();
    let outputs = executor
        .action(action_name, state.spendable)
        .map_err(|err| anyhow!("proving failed: {err}"))?;
    let elapsed = start.elapsed();

    println!();
    println!("Proved in {:.2}s", elapsed.as_secs_f64());
    println!(
        "tx_final: {:#}",
        outputs.tx.ctx.commitment()
    );
    println!("Output objects ({}):", outputs.objs.len());
    for (i, obj) in outputs.objs.iter().enumerate() {
        println!("  [{i}] commitment={:#}", obj.obj.commitment());
    }

    Ok(())
}

pub fn plan(target: &Path, action_name: &str, mode: PlanOutput) -> Result<()> {
    let (manifest, script) = load_target(target)?;
    let module = load_sdk_module(&manifest, &script)?;
    let action = module
        .actions()
        .iter()
        .find(|a| a.name == action_name)
        .ok_or_else(|| anyhow!("no action named {action_name} in this plugin"))?;

    let input_classes: Vec<String> = action.total_inputs().map(|r| r.class.clone()).collect();
    let output_classes: Vec<String> = action.total_outputs().map(|r| r.class.clone()).collect();

    let mut minted = Vec::with_capacity(input_classes.len());
    for class in &input_classes {
        let obj = crate::fixtures::mint_class(&module, class)?;
        minted.push(obj);
    }
    let state = crate::fixtures::build_synthetic_state(&minted)?;
    let executor = module.executor(true, state.grounding_witness.clone());
    let plan = executor
        .plan_action(action_name, state.spendable)
        .map_err(|err| anyhow!("planning failed: {err}"))?;
    let aliases = build_alias_map(&module);

    match mode {
        PlanOutput::DotCompressed => print_dep_graph_dot(
            action_name,
            &input_classes,
            &output_classes,
            &plan,
            &aliases,
            true,
        ),
        PlanOutput::DotFull => print_dep_graph_dot(
            action_name,
            &input_classes,
            &output_classes,
            &plan,
            &aliases,
            false,
        ),
        PlanOutput::MermaidCompressed => print_dep_graph_mermaid(
            action_name,
            &input_classes,
            &output_classes,
            &plan,
            &aliases,
            true,
            false,
        ),
        PlanOutput::MermaidFull => print_dep_graph_mermaid(
            action_name,
            &input_classes,
            &output_classes,
            &plan,
            &aliases,
            false,
            false,
        ),
        PlanOutput::MermaidLinkCompressed => print_dep_graph_mermaid(
            action_name,
            &input_classes,
            &output_classes,
            &plan,
            &aliases,
            true,
            true,
        ),
        PlanOutput::MermaidLinkFull => print_dep_graph_mermaid(
            action_name,
            &input_classes,
            &output_classes,
            &plan,
            &aliases,
            false,
            true,
        ),
        PlanOutput::Text(sections) => {
            let mut printed_above = false;
            if sections.contains(&PlanSection::Header) {
                println!("Plan: {}", action_name);
                println!("  Inputs ({}):", input_classes.len());
                for class in &input_classes {
                    println!("    - {class}");
                }
                println!("  Outputs ({}):", output_classes.len());
                for class in &output_classes {
                    println!("    - {class}");
                }
                printed_above = true;
            }
            if sections.contains(&PlanSection::Summary) {
                if printed_above {
                    println!();
                }
                print!("{}", plan.solved.solution_breakdown());
                printed_above = true;
            }
            if sections.contains(&PlanSection::Totals) {
                if printed_above {
                    println!();
                }
                print_custom_predicate_totals(&plan, &aliases);
                printed_above = true;
            }
            if sections.contains(&PlanSection::Deps) {
                if printed_above {
                    println!();
                }
                print_dep_graph(&plan, &aliases);
            }
        }
    }

    Ok(())
}

fn print_dep_graph(plan: &sdk::PlanData, aliases: &HashMap<Hash, String>) {
    use pod2::frontend::AbstractDep;

    let shape = plan.solved.input_shape();
    let output = plan.solved.solution();
    let n_original = plan.statements.len();

    println!("Statement dep graph:");
    for (pod_idx, stmts) in output.pod_statements.iter().enumerate() {
        let role = if output.is_output_pod(pod_idx) {
            "output"
        } else {
            "intermediate"
        };
        println!("  POD {pod_idx} ({role}):");
        for &s in stmts {
            // The shape's `dep_edges` is augmented at solve time with
            // synthetic republish entries at indices >= n_original;
            // those don't correspond to user statements and we just
            // skip them in the dep-graph view.
            if s >= n_original {
                continue;
            }
            let label = statement_label(&plan.statements[s], aliases);
            let deps: Vec<String> = shape.dep_edges[s]
                .iter()
                .filter_map(|dep| match dep {
                    AbstractDep::Internal(idx) => Some(format!("[{idx}]")),
                    AbstractDep::External { pod, statement } => {
                        Some(format!("ext{pod}:{statement}"))
                    }
                })
                .collect();
            if deps.is_empty() {
                println!("    [{s}] {label}");
            } else {
                println!("    [{s}] {label} <- {}", deps.join(", "));
            }
        }
    }
}

/// Map each imported module's batch hash to its declared alias (e.g.
/// `txlib`'s batch id -> `"tx"`). Used to qualify foreign predicate
/// names in label rendering. The local module's batch is *not* in the
/// dependency list, so its customs come out unprefixed naturally.
fn build_alias_map(module: &SdkModule) -> HashMap<Hash, String> {
    let mut map = HashMap::new();
    for dep in module.dependencies() {
        if let Dependency::Module { name, hash } = dep {
            map.insert(*hash, name.clone());
        }
    }
    map
}

fn format_custom_name(
    custom_ref: &pod2::middleware::CustomPredicateRef,
    aliases: &HashMap<Hash, String>,
) -> String {
    let name = &custom_ref.predicate().name;
    let batch_id = custom_ref.batch.id();
    match aliases.get(&batch_id) {
        Some(alias) => format!("{alias}::{name}"),
        None => name.clone(),
    }
}

fn statement_label(
    stmt: &pod2::middleware::Statement,
    aliases: &HashMap<Hash, String>,
) -> String {
    match stmt.predicate() {
        Predicate::Native(n) => format!("{n}"),
        Predicate::Custom(c) => format_custom_name(&c, aliases),
        Predicate::Intro(i) => i.name.clone(),
        Predicate::BatchSelf(idx) => format!("batch_self_{idx}"),
    }
}

/// Count each distinct custom predicate by occurrences in the plan's
/// statement list. Native and Intro statements are excluded — they're
/// already covered by the solution breakdown's resource categories.
/// Statements produced by `ReplaceValueWithEntry` are skipped so a
/// custom predicate that gets rewritten doesn't get double-counted.
/// Imported predicates use their qualified `<alias>::<name>` form.
fn print_custom_predicate_totals(plan: &sdk::PlanData, aliases: &HashMap<Hash, String>) {
    let rewrites = build_rewrite_source(plan);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, stmt) in plan.statements.iter().enumerate() {
        if rewrites.contains_key(&idx) {
            continue;
        }
        if let Predicate::Custom(custom_ref) = stmt.predicate() {
            *counts
                .entry(format_custom_name(&custom_ref, aliases))
                .or_insert(0) += 1;
        }
    }
    if counts.is_empty() {
        return;
    }
    // Sort by count desc, name asc (BTreeMap iteration gives name-asc;
    // stable sort by -count preserves alphabetical tie-break).
    let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    println!("Custom predicates (by usage):");
    for (name, count) in sorted {
        println!("  {count:>3}  {name}");
    }
}

/// Map each statement index that was produced by `ReplaceValueWithEntry`
/// to the index of the statement it rewrites. The source statement is
/// the last `OperationArg` (see `Operation::replace_value_with_entry`
/// in pod2).
fn build_rewrite_source(plan: &sdk::PlanData) -> BTreeMap<usize, usize> {
    use pod2::frontend::OperationArg;
    use pod2::middleware::{NativeOperation, OperationType};

    let mut out: BTreeMap<usize, usize> = BTreeMap::new();
    for (idx, op) in plan.operations.iter().enumerate() {
        if !matches!(
            op.0,
            OperationType::Native(NativeOperation::ReplaceValueWithEntry)
        ) {
            continue;
        }
        let source_stmt = match op.1.last() {
            Some(OperationArg::Statement(s)) => s,
            _ => continue,
        };
        if let Some(src_idx) = plan.statements.iter().position(|s| s == source_stmt) {
            out.insert(idx, src_idx);
        }
    }
    out
}

fn print_dep_graph_dot(
    action_name: &str,
    input_classes: &[String],
    output_classes: &[String],
    plan: &sdk::PlanData,
    aliases: &HashMap<Hash, String>,
    compressed: bool,
) {
    use pod2::frontend::AbstractDep;

    let shape = plan.solved.input_shape();
    let output = plan.solved.solution();
    let n_original = plan.statements.len();

    // In compressed mode, hide statements that are anchored-key
    // rewrites of an earlier statement and redirect their consumers to
    // the source. The full view shows every statement so it doesn't
    // build the map.
    let rewrite_source = if compressed {
        build_rewrite_source(plan)
    } else {
        BTreeMap::new()
    };
    // Follow rewrite chains so the redirect target is always a
    // "real" statement (not another rewrite). Each chain bottoms out
    // in O(chain_length) iterations.
    let resolve = |mut idx: usize| -> usize {
        while let Some(&src) = rewrite_source.get(&idx) {
            idx = src;
        }
        idx
    };

    // In compressed mode, a statement is a graph node iff its predicate
    // is Custom or Intro AND it isn't a rewrite of an earlier statement.
    let is_node = |s: usize| -> bool {
        if s >= n_original {
            return false;
        }
        if !compressed {
            return true;
        }
        if rewrite_source.contains_key(&s) {
            return false;
        }
        matches!(
            plan.statements[s].predicate(),
            Predicate::Custom(_) | Predicate::Intro(_)
        )
    };

    // For each visible node, the set of visible producer indices and
    // external (pod, stmt) refs. In compressed mode this walks through
    // hidden Native statements until reaching a visible producer, and
    // resolves rewritten statements to their source.
    let producer_set =
        |s: usize| -> (BTreeSet<usize>, BTreeSet<(usize, usize)>) {
            let mut internal: BTreeSet<usize> = BTreeSet::new();
            let mut external: BTreeSet<(usize, usize)> = BTreeSet::new();
            let mut visited: BTreeSet<usize> = BTreeSet::new();
            let mut queue: Vec<&AbstractDep> = shape.dep_edges[s].iter().collect();
            while let Some(dep) = queue.pop() {
                match dep {
                    AbstractDep::Internal(d) => {
                        let d = resolve(*d);
                        if !visited.insert(d) {
                            continue;
                        }
                        if d >= n_original {
                            continue;
                        }
                        if d == s {
                            // Self-loops can appear if a statement is
                            // both a rewrite of itself's predecessor
                            // and also references that predecessor;
                            // resolve collapses both ends.
                            continue;
                        }
                        if is_node(d) {
                            internal.insert(d);
                        } else if compressed {
                            queue.extend(shape.dep_edges[d].iter());
                        }
                    }
                    AbstractDep::External { pod, statement } => {
                        external.insert((*pod, *statement));
                    }
                }
            }
            (internal, external)
        };

    let mut out = String::new();
    let suffix = if compressed { "" } else { "_full" };
    out.push_str(&format!(
        "digraph plan_{}{suffix} {{\n",
        sanitize(action_name)
    ));
    out.push_str("  rankdir=TB;\n");
    out.push_str("  node [fontname=\"Helvetica\", shape=box, style=\"rounded,filled\", fillcolor=\"#f6f8fa\"];\n");
    out.push_str("  edge [fontname=\"Helvetica\", fontsize=10];\n");
    out.push_str("  concentrate=true;\n");
    let mode_tag = if compressed { "compressed" } else { "full" };
    out.push_str(&format!(
        "  label=\"{} ({mode_tag}) -- inputs: [{}], outputs: [{}]\";\n",
        action_name,
        input_classes.join(", "),
        output_classes.join(", "),
    ));
    out.push_str("  labelloc=t;\n\n");

    // Per-POD clusters group statements visually. In compressed mode
    // a POD with no visible nodes is skipped entirely.
    for (pod_idx, stmts) in output.pod_statements.iter().enumerate() {
        let visible: Vec<usize> = stmts.iter().copied().filter(|&s| is_node(s)).collect();
        if visible.is_empty() {
            continue;
        }
        let role = if output.is_output_pod(pod_idx) {
            "output"
        } else {
            "intermediate"
        };
        out.push_str(&format!("  subgraph cluster_pod_{pod_idx} {{\n"));
        out.push_str(&format!("    label=\"POD {pod_idx} ({role})\";\n"));
        out.push_str("    style=dashed;\n");
        for s in visible {
            let label = statement_label(&plan.statements[s], aliases);
            out.push_str(&format!("    s{s} [label=\"[{s}] {label}\"];\n"));
        }
        out.push_str("  }\n");
    }
    out.push('\n');

    // External-pod statements live in a single dotted cluster with
    // distinctive node styling.
    let mut ext_refs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for s in 0..n_original {
        if !is_node(s) {
            continue;
        }
        let (_internal, external) = producer_set(s);
        ext_refs.extend(external);
    }
    if !ext_refs.is_empty() {
        out.push_str("  subgraph cluster_external {\n");
        out.push_str("    label=\"external pods\";\n");
        out.push_str("    style=dotted;\n");
        for (pod, stmt) in &ext_refs {
            out.push_str(&format!(
                "    ext{pod}_{stmt} [label=\"ext{pod}:{stmt}\", shape=note, fillcolor=\"#fff5b7\"];\n",
            ));
        }
        out.push_str("  }\n\n");
    }

    // Edges: producer -> consumer.
    for s in 0..n_original {
        if !is_node(s) {
            continue;
        }
        let (internal, external) = producer_set(s);
        for d in internal {
            out.push_str(&format!("  s{d} -> s{s};\n"));
        }
        for (pod, stmt) in external {
            out.push_str(&format!("  ext{pod}_{stmt} -> s{s};\n"));
        }
    }

    out.push_str("}\n");
    print!("{}", out);
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Structural view of the dep graph, abstracted away from the output
/// format. Built once per plan + compression-mode and consumed by
/// either the DOT or Mermaid renderer.
struct GraphView {
    /// Per-POD list of statement indices visible in this view.
    pod_visible: Vec<Vec<usize>>,
    /// Per-visible-node, the set of internal producer indices and
    /// external (pod, stmt) refs after Native folding and rewrite
    /// resolution.
    edges: BTreeMap<usize, (BTreeSet<usize>, BTreeSet<(usize, usize)>)>,
    /// Distinct external refs across the whole view.
    external_refs: BTreeSet<(usize, usize)>,
    compressed: bool,
}

fn build_graph_view(plan: &sdk::PlanData, compressed: bool) -> GraphView {
    use pod2::frontend::AbstractDep;

    let shape = plan.solved.input_shape();
    let output = plan.solved.solution();
    let n_original = plan.statements.len();

    let rewrite_source = if compressed {
        build_rewrite_source(plan)
    } else {
        BTreeMap::new()
    };
    let resolve = |mut idx: usize| -> usize {
        while let Some(&src) = rewrite_source.get(&idx) {
            idx = src;
        }
        idx
    };
    let is_node = |s: usize| -> bool {
        if s >= n_original {
            return false;
        }
        if !compressed {
            return true;
        }
        if rewrite_source.contains_key(&s) {
            return false;
        }
        matches!(
            plan.statements[s].predicate(),
            Predicate::Custom(_) | Predicate::Intro(_)
        )
    };

    let pod_visible: Vec<Vec<usize>> = output
        .pod_statements
        .iter()
        .map(|stmts| stmts.iter().copied().filter(|&s| is_node(s)).collect())
        .collect();

    let mut edges: BTreeMap<usize, (BTreeSet<usize>, BTreeSet<(usize, usize)>)> = BTreeMap::new();
    let mut external_refs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for s in 0..n_original {
        if !is_node(s) {
            continue;
        }
        let mut internal: BTreeSet<usize> = BTreeSet::new();
        let mut external: BTreeSet<(usize, usize)> = BTreeSet::new();
        let mut visited: BTreeSet<usize> = BTreeSet::new();
        let mut queue: Vec<&AbstractDep> = shape.dep_edges[s].iter().collect();
        while let Some(dep) = queue.pop() {
            match dep {
                AbstractDep::Internal(d) => {
                    let d = resolve(*d);
                    if !visited.insert(d) {
                        continue;
                    }
                    if d >= n_original || d == s {
                        continue;
                    }
                    if is_node(d) {
                        internal.insert(d);
                    } else if compressed {
                        queue.extend(shape.dep_edges[d].iter());
                    }
                }
                AbstractDep::External { pod, statement } => {
                    external.insert((*pod, *statement));
                    external_refs.insert((*pod, *statement));
                }
            }
        }
        edges.insert(s, (internal, external));
    }

    GraphView {
        pod_visible,
        edges,
        external_refs,
        compressed,
    }
}

fn print_dep_graph_mermaid(
    action_name: &str,
    input_classes: &[String],
    output_classes: &[String],
    plan: &sdk::PlanData,
    aliases: &HashMap<Hash, String>,
    compressed: bool,
    as_link: bool,
) {
    let source = build_mermaid_source(
        action_name,
        input_classes,
        output_classes,
        plan,
        aliases,
        compressed,
    );
    if as_link {
        match mermaid_live_url(&source) {
            Ok(url) => println!("{url}"),
            Err(err) => {
                eprintln!("failed to build mermaid.live URL: {err}");
                print!("{source}");
            }
        }
    } else {
        print!("{source}");
    }
}

fn build_mermaid_source(
    action_name: &str,
    input_classes: &[String],
    output_classes: &[String],
    plan: &sdk::PlanData,
    aliases: &HashMap<Hash, String>,
    compressed: bool,
) -> String {
    let view = build_graph_view(plan, compressed);
    let output = plan.solved.solution();
    let mode_tag = if view.compressed { "compressed" } else { "full" };

    let mut out = String::new();
    out.push_str("flowchart TD\n");
    // Mermaid renders a title via the `flowchart` directive's
    // `subgraph`-style label or via the `%%{init: {...}}%%` directive;
    // a leading comment is the simplest portable approach.
    out.push_str(&format!(
        "%% {} ({}) -- inputs: [{}], outputs: [{}]\n",
        action_name,
        mode_tag,
        input_classes.join(", "),
        output_classes.join(", "),
    ));

    for (pod_idx, visible) in view.pod_visible.iter().enumerate() {
        if visible.is_empty() {
            continue;
        }
        let role = if output.is_output_pod(pod_idx) {
            "output"
        } else {
            "intermediate"
        };
        out.push_str(&format!(
            "  subgraph pod{pod_idx}[\"POD {pod_idx} ({role})\"]\n"
        ));
        for &s in visible {
            let label = statement_label(&plan.statements[s], aliases);
            out.push_str(&format!("    s{s}[\"[{s}] {}\"]\n", escape_mermaid(&label)));
        }
        out.push_str("  end\n");
    }

    if !view.external_refs.is_empty() {
        out.push_str("  subgraph ext_pods[\"external pods\"]\n");
        for (pod, stmt) in &view.external_refs {
            out.push_str(&format!("    ext{pod}_{stmt}[\"ext{pod}:{stmt}\"]\n"));
        }
        out.push_str("  end\n");
    }

    out.push_str("\n");
    for (&s, (internal, external)) in &view.edges {
        for d in internal {
            out.push_str(&format!("  s{d} --> s{s}\n"));
        }
        for (pod, stmt) in external {
            out.push_str(&format!("  ext{pod}_{stmt} --> s{s}\n"));
        }
    }

    // Style external nodes distinctively.
    if !view.external_refs.is_empty() {
        out.push_str("  classDef external fill:#fff5b7,stroke:#aa8800;\n");
        for (pod, stmt) in &view.external_refs {
            out.push_str(&format!("  class ext{pod}_{stmt} external;\n"));
        }
    }

    out
}

/// Build a `https://mermaid.live/edit#pako:<encoded>` URL for `source`.
/// Format matches what mermaid.live's `pako:` decoder expects:
/// JSON-encode `{code, mermaid: theme JSON, updateEditor: ...}`, zlib-
/// compress (deflate with zlib wrapper) at level 9, then base64-encode.
fn mermaid_live_url(source: &str) -> Result<String> {
    use base64::Engine;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    // Minimal state object the editor accepts. `updateDiagram` lets
    // edits re-render automatically; `autoSync` keeps the URL in sync
    // with the editor.
    let state = serde_json::json!({
        "code": source,
        "mermaid": "{\n  \"theme\": \"default\"\n}",
        "autoSync": true,
        "updateDiagram": true,
        "panZoom": true,
    });
    let json = serde_json::to_vec(&state)?;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&json)
        .map_err(|err| anyhow!("zlib write: {err}"))?;
    let compressed = encoder
        .finish()
        .map_err(|err| anyhow!("zlib finish: {err}"))?;

    let encoded = base64::engine::general_purpose::STANDARD.encode(&compressed);
    Ok(format!("https://mermaid.live/edit#pako:{encoded}"))
}

/// Mermaid labels in double quotes still need `"` and backslashes
/// escaped. Brackets and other special chars are fine inside quoted
/// strings.
fn escape_mermaid(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `pexe inspect graph`.
///
/// Emits Graphviz DOT for the action/class relationship graph. Class
/// nodes (boxes) sit at the perimeter; action nodes (ellipses) are
/// connected via `in` / `out` / `mutate` edges. Pipe the output to
/// `dot -Tsvg` (or your renderer of choice) to produce an image.
pub fn graph(target: &Path) -> Result<()> {
    let (manifest, script) = load_target(target)?;
    let module = load_sdk_module(&manifest, &script)?;

    let mut out = String::new();
    out.push_str("digraph pexe {\n");
    out.push_str("  rankdir=LR;\n");
    out.push_str("  node [fontname=\"Helvetica\"];\n");
    out.push_str("  edge [fontname=\"Helvetica\", fontsize=10];\n\n");

    out.push_str("  // classes\n");
    for class in module.classes() {
        out.push_str(&format!(
            "  \"class_{}\" [label=\"{}\", shape=box, style=filled, fillcolor=lightblue];\n",
            class.name, class.name,
        ));
    }
    out.push('\n');

    out.push_str("  // actions\n");
    for action in module.actions() {
        out.push_str(&format!(
            "  \"action_{}\" [label=\"{}\", shape=ellipse];\n",
            action.name, action.name,
        ));
    }
    out.push('\n');

    out.push_str("  // edges\n");
    for action in module.actions() {
        let inputs: BTreeSet<String> = action
            .local_inputs()
            .map(|r| r.class.clone())
            .collect();
        let outputs: BTreeSet<String> = action
            .local_outputs()
            .map(|r| r.class.clone())
            .collect();
        let mutates: BTreeSet<&String> = inputs.intersection(&outputs).collect();

        for class in &inputs {
            if mutates.contains(class) {
                continue;
            }
            out.push_str(&format!(
                "  \"class_{class}\" -> \"action_{action}\" [label=\"in\"];\n",
                action = action.name,
            ));
        }
        for class in &outputs {
            if mutates.contains(class) {
                continue;
            }
            out.push_str(&format!(
                "  \"action_{}\" -> \"class_{class}\" [label=\"out\"];\n",
                action.name,
            ));
        }
        for class in mutates {
            out.push_str(&format!(
                "  \"action_{}\" -> \"class_{class}\" [label=\"mutate\", dir=both, color=darkorange];\n",
                action.name,
            ));
        }
    }

    out.push_str("}\n");
    print!("{}", out);
    Ok(())
}

/// `pexe inspect classes`.
///
/// Render each class's state-space signature in "Notation A" — a
/// pod2-typed record listing application fields with literal narrowing
/// where possible and a crypto appendix for VDF/PoW-derived fields.
pub fn classes(target: &Path, class_filter: Option<&str>) -> Result<()> {
    let (manifest, script) = load_target(target)?;
    let module = load_sdk_module(&manifest, &script)?;
    let batch: &std::sync::Arc<CustomPredicateBatch> = &module.module().batch;

    let mut first = true;
    let mut matched = false;
    for class in module.classes() {
        if let Some(name) = class_filter {
            if class.name != name {
                continue;
            }
        }
        matched = true;
        if !first {
            println!();
        }
        first = false;
        let signature = derive_class_signature(&module, batch, &class.name);
        println!("{}", render_signature(&signature));
    }
    if let Some(name) = class_filter {
        if !matched {
            return Err(anyhow!("no class named {name} in this plugin"));
        }
    }
    Ok(())
}

/// Per-class collected info: fields and crypto provenance flags.
pub(crate) struct ClassSignature {
    pub(crate) name: String,
    pub(crate) fields: BTreeMap<String, FieldInfo>,
    pub(crate) uses_vdf: bool,
    pub(crate) uses_pow: bool,
}

#[derive(Default)]
pub(crate) struct FieldInfo {
    /// Literal string values ever assigned to this field.
    pub(crate) string_literals: BTreeSet<String>,
    /// Integer literals ever assigned.
    pub(crate) int_literals: BTreeSet<i64>,
    /// True if any assignment was a wildcard whose source is a VDF intro.
    pub(crate) from_vdf: bool,
    /// True if any assignment was a wildcard with no other inferable provenance.
    pub(crate) from_witness: bool,
}

/// Re-export of [`derive_class_signature`] under a name that signals
/// it's for use by external fixture builders. Same semantics; same
/// return type.
pub(crate) fn derive_class_signature_for_fixture(
    module: &SdkModule,
    batch: &std::sync::Arc<CustomPredicateBatch>,
    class_name: &str,
) -> ClassSignature {
    derive_class_signature(module, batch, class_name)
}

fn derive_class_signature(
    module: &SdkModule,
    batch: &std::sync::Arc<CustomPredicateBatch>,
    class_name: &str,
) -> ClassSignature {
    let mut sig = ClassSignature {
        name: class_name.to_string(),
        fields: BTreeMap::new(),
        uses_vdf: false,
        uses_pow: false,
    };
    let class_hash = match module.class_hash(class_name) {
        Some(h) => h,
        None => return sig,
    };

    for predicate in batch.predicates() {
        // Iterate the inlined scope, not just the direct body. Some
        // actions get split so the TxInsert / TxMutate event lands in
        // a `<Action>_N` helper while the dict-construction (Contains /
        // ContainerUpdate linking output[i] to its inner wildcard)
        // stays in the caller. We need both to be visible to one
        // chain-tracing pass.
        let scope = inline_action(predicate, batch);
        for stmt in &scope {
            let focused = match tx_producer_focused(stmt, batch, class_hash) {
                Some(arg) => arg,
                None => continue,
            };
            let chain = trace_state_chain(&scope, &focused);
            let vdf_producers = collect_intro_outputs(&scope, &VDF_VD_HASH);
            collect_fields_into_scope(&scope, &chain, &vdf_producers, &mut sig);
            if !vdf_producers.is_empty() {
                sig.uses_vdf = true;
            }
            if scope_uses_intro(&scope, &LT_EQ_U256_VD_HASH) {
                sig.uses_pow = true;
            }
        }
    }

    sig
}

static VDF_VD_HASH: LazyLock<Hash> = LazyLock::new(|| *vdfpod::STANDARD_VDF_VD_HASH);
static LT_EQ_U256_VD_HASH: LazyLock<Hash> =
    LazyLock::new(|| *lt_eq_u256_pod::STANDARD_LT_EQ_U256_VD_HASH);

/// Inline an action predicate's `BatchSelf(N)` calls into a flat list
/// of statements, substituting the helper's parameter wildcards with
/// the call-site args and offsetting the helper's private wildcards so
/// they cannot collide with the caller's. After this, the returned
/// statements all share one wildcard namespace: structural equality on
/// `StatementTmplArg` is sound for chain tracing.
fn inline_action(
    predicate: &pod2::middleware::CustomPredicate,
    batch: &std::sync::Arc<CustomPredicateBatch>,
) -> Vec<StatementTmpl> {
    let mut out: Vec<StatementTmpl> = predicate.statements().to_vec();
    // Offset slots are spaced large enough that helpers never collide
    // with the caller's wildcards or with each other. 10_000 is well
    // beyond the wildcard count of any plausible predicate.
    let caller_offset: usize = 0;
    let mut next_helper_offset: usize = caller_offset + 10_000;
    for stmt in predicate.statements() {
        if let PredicateOrWildcard::Predicate(Predicate::BatchSelf(idx)) = &stmt.pred_or_wc {
            let Some(sub) = batch.predicates().get(*idx) else {
                continue;
            };
            let bindings: &[StatementTmplArg] = &stmt.args;
            let offset = next_helper_offset;
            next_helper_offset += 10_000;
            for sub_stmt in sub.statements() {
                out.push(substitute_statement(sub_stmt, bindings, offset));
            }
        }
    }
    out
}

fn substitute_statement(
    stmt: &StatementTmpl,
    bindings: &[StatementTmplArg],
    offset: usize,
) -> StatementTmpl {
    let args = stmt
        .args
        .iter()
        .map(|a| substitute_arg(a, bindings, offset))
        .collect();
    StatementTmpl {
        pred_or_wc: stmt.pred_or_wc.clone(),
        args,
    }
}

fn substitute_arg(
    arg: &StatementTmplArg,
    bindings: &[StatementTmplArg],
    offset: usize,
) -> StatementTmplArg {
    match arg {
        StatementTmplArg::Wildcard(wc) => {
            if wc.index < bindings.len() {
                bindings[wc.index].clone()
            } else {
                StatementTmplArg::Wildcard(Wildcard {
                    name: wc.name.clone(),
                    index: wc.index + offset,
                })
            }
        }
        StatementTmplArg::AnchoredKey(wc, key) => {
            if wc.index < bindings.len() {
                // The wildcard is a parameter; substitute it. If the
                // call-site binding is itself a Wildcard, we can rewrite
                // the AnchoredKey to reference the caller's wildcard.
                // For other binding shapes (Literal, AnchoredKey,
                // SelfPredicateHash) the construct isn't expressible and
                // we leave the arg as-is — best-effort fallback.
                match &bindings[wc.index] {
                    StatementTmplArg::Wildcard(w) => {
                        StatementTmplArg::AnchoredKey(w.clone(), key.clone())
                    }
                    _ => arg.clone(),
                }
            } else {
                StatementTmplArg::AnchoredKey(
                    Wildcard {
                        name: wc.name.clone(),
                        index: wc.index + offset,
                    },
                    key.clone(),
                )
            }
        }
        _ => arg.clone(),
    }
}


/// If `stmt` is a txlib producer event (TxInsert or TxMutate) whose
/// `@self_predicate(IsX)` arg resolves to the given `class_hash`, return
/// the focused state arg. Compares predicates by hash, not name, so a
/// rename of TxInsert/TxMutate upstream is harmless. TxDelete is
/// excluded because deletion doesn't define the object's shape.
fn tx_producer_focused(
    stmt: &StatementTmpl,
    batch: &std::sync::Arc<CustomPredicateBatch>,
    class_hash: Hash,
) -> Option<StatementTmplArg> {
    let custom_ref = match &stmt.pred_or_wc {
        PredicateOrWildcard::Predicate(Predicate::Custom(c)) => c,
        _ => return None,
    };
    let event_hash = Predicate::Custom(custom_ref.clone()).hash();
    if event_hash != *TX_INSERT_HASH && event_hash != *TX_MUTATE_HASH {
        return None;
    }
    // TxInsert(chain, chain0, state, type_hash);
    // TxMutate(chain, chain0, new_state, type_hash, old_state).
    // The third arg is always the focused-state for the producer event.
    let state = stmt.args.get(2)?.clone();
    let actual_class_hash = stmt.args.iter().find_map(|a| match a {
        StatementTmplArg::SelfPredicateHash(idx) => batch
            .predicate_ref_by_index(*idx)
            .map(|cref| Predicate::Custom(cref).hash()),
        _ => None,
    })?;
    if actual_class_hash != class_hash {
        return None;
    }
    Some(state)
}

/// Set of dict states semantically equivalent to `focused` within the
/// inlined scope. Follows dict-transition `new = f(old)` links until
/// fixed point. After inlining, wildcards have a single global scope
/// so structural equality on `StatementTmplArg` is correct.
fn trace_state_chain(
    scope: &[StatementTmpl],
    focused: &StatementTmplArg,
) -> HashSet<StatementTmplArg> {
    let mut chain: HashSet<StatementTmplArg> = HashSet::new();
    chain.insert(focused.clone());
    loop {
        let mut grew = false;
        for stmt in scope {
            if let Some((new, old)) = dict_transition(stmt) {
                if chain.contains(&new) || chain.contains(&old) {
                    if chain.insert(new.clone()) {
                        grew = true;
                    }
                    if chain.insert(old.clone()) {
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }
    chain
}

/// If `stmt` is a dict transition op (Insert/Update/Delete), return
/// `(new_state, old_state)`. Matches the elaborated middleware forms
/// (`ContainerInsert`/`ContainerUpdate`/`ContainerDelete`) since
/// `DictInsert` etc. are syntactic sugar lowered during compilation.
fn dict_transition(stmt: &StatementTmpl) -> Option<(StatementTmplArg, StatementTmplArg)> {
    let native = native_predicate(&stmt.pred_or_wc)?;
    match native {
        NativePredicate::ContainerInsert
        | NativePredicate::ContainerUpdate
        | NativePredicate::ContainerDelete
        | NativePredicate::DictInsert
        | NativePredicate::DictUpdate
        | NativePredicate::DictDelete => {
            let new = stmt.args.first()?.clone();
            let old = stmt.args.get(1)?.clone();
            Some((new, old))
        }
        _ => None,
    }
}

fn native_predicate(pred_or_wc: &PredicateOrWildcard) -> Option<NativePredicate> {
    match pred_or_wc {
        PredicateOrWildcard::Predicate(Predicate::Native(n)) => Some(*n),
        _ => None,
    }
}

/// Find wildcard *names* that a named intro produces (e.g. "Vdf"'s
/// third arg is the work output). Cross-predicate, name-based to match
/// the chain tracing strategy.
/// True if any statement in `scope` invokes the intro predicate with
/// the given verifier-data hash. Hash-based so a name change in the
/// intro pod registration doesn't silently break detection.
fn scope_uses_intro(scope: &[StatementTmpl], vd_hash: &Hash) -> bool {
    scope.iter().any(|stmt| {
        matches!(
            &stmt.pred_or_wc,
            PredicateOrWildcard::Predicate(Predicate::Intro(intro))
                if &intro.verifier_data_hash == vd_hash
        )
    })
}

/// Collect the output wildcards produced by an intro identified by its
/// verifier-data hash. The convention is that the *last* arg of an
/// intro statement is its output wildcard (e.g., Vdf's `work`).
fn collect_intro_outputs(scope: &[StatementTmpl], vd_hash: &Hash) -> HashSet<Wildcard> {
    let mut out = HashSet::new();
    for stmt in scope {
        if let PredicateOrWildcard::Predicate(Predicate::Intro(intro)) = &stmt.pred_or_wc {
            if &intro.verifier_data_hash == vd_hash {
                if let Some(StatementTmplArg::Wildcard(wc)) = stmt.args.last() {
                    out.insert(wc.clone());
                }
            }
        }
    }
    out
}

fn collect_fields_into_scope(
    scope: &[StatementTmpl],
    chain: &HashSet<StatementTmplArg>,
    vdf_producers: &HashSet<Wildcard>,
    sig: &mut ClassSignature,
) {
    for stmt in scope {
        let native = match native_predicate(&stmt.pred_or_wc) {
            Some(n) => n,
            None => continue,
        };
        let (state_arg, key_arg, value_arg) = match native {
            NativePredicate::Contains | NativePredicate::DictContains => {
                (stmt.args.first(), stmt.args.get(1), stmt.args.get(2))
            }
            NativePredicate::ContainerInsert
            | NativePredicate::ContainerUpdate
            | NativePredicate::DictInsert
            | NativePredicate::DictUpdate => {
                (stmt.args.first(), stmt.args.get(2), stmt.args.get(3))
            }
            _ => continue,
        };
        let Some(state_arg) = state_arg else {
            continue;
        };
        let is_transition = matches!(
            native,
            NativePredicate::ContainerInsert
                | NativePredicate::ContainerUpdate
                | NativePredicate::DictInsert
                | NativePredicate::DictUpdate
        );
        let mut in_chain = chain.contains(state_arg);
        if !in_chain && is_transition {
            if let Some(old) = stmt.args.get(1) {
                in_chain = chain.contains(old);
            }
        }
        if !in_chain {
            continue;
        }
        let (Some(key_arg), Some(value_arg)) = (key_arg, value_arg) else {
            continue;
        };
        let field_name = match literal_string(key_arg) {
            Some(s) => s,
            None => continue,
        };
        let info = sig.fields.entry(field_name).or_default();
        record_value(value_arg, vdf_producers, info);
    }
}

fn literal_string(arg: &StatementTmplArg) -> Option<String> {
    match arg {
        StatementTmplArg::Literal(v) => v.as_string(),
        _ => None,
    }
}

fn record_value(arg: &StatementTmplArg, vdf_producers: &HashSet<Wildcard>, info: &mut FieldInfo) {
    match arg {
        StatementTmplArg::Literal(v) => {
            if let Some(s) = v.as_string() {
                info.string_literals.insert(s);
            } else if let Some(i) = v.as_int() {
                info.int_literals.insert(i);
            } else {
                info.from_witness = true;
            }
        }
        StatementTmplArg::Wildcard(wc) => {
            if vdf_producers.contains(wc) {
                info.from_vdf = true;
            } else {
                info.from_witness = true;
            }
        }
        _ => {
            info.from_witness = true;
        }
    }
}

fn render_signature(sig: &ClassSignature) -> String {
    let mut out = String::new();
    out.push_str(&format!("class {} {{\n", sig.name));
    let mut field_lines: Vec<(String, String)> = Vec::new();
    for (name, info) in &sig.fields {
        field_lines.push((name.clone(), render_field_value(info)));
    }
    let name_width = field_lines
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    for (name, value) in &field_lines {
        out.push_str(&format!("  {:width$}  {}\n", name, value, width = name_width));
    }
    if sig.uses_vdf || sig.uses_pow {
        out.push_str("  // identity:");
        if sig.uses_pow {
            out.push_str(" PoW (lt_eq_u256)");
        }
        if sig.uses_vdf {
            out.push_str(" VDF");
        }
        out.push('\n');
    }
    out.push('}');
    out
}

fn render_field_value(info: &FieldInfo) -> String {
    let strings: Vec<String> = info.string_literals.iter().cloned().collect();
    let ints: Vec<i64> = info.int_literals.iter().copied().collect();

    let parts: Vec<String> = match (strings.is_empty(), ints.is_empty()) {
        (false, true) => {
            let union = strings
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(" | ");
            vec![union]
        }
        (true, false) => {
            let union = ints
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" | ");
            vec![format!("Int  // initial: {union}")]
        }
        (false, false) => {
            let strs = strings
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(" | ");
            let nums = ints
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" | ");
            vec![format!("{strs} | {nums}")]
        }
        (true, true) => Vec::new(),
    };

    if !parts.is_empty() {
        return parts.into_iter().next().unwrap();
    }

    if info.from_vdf {
        "Raw  // VDF-derived".to_string()
    } else if info.from_witness {
        "Raw  // witness".to_string()
    } else {
        "?".to_string()
    }
}

/// Extract the text of a top-level predicate definition by name.
///
/// Top-level predicates begin at column 0 with `<Name>(` and continue
/// through their closing `)` at column 0. Walks the source line by line,
/// returning the slice from the matching `Name(` line through the next
/// `)` line at column 0.
fn find_predicate_block<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let header_prefix = format!("{name}(");
    let mut start: Option<usize> = None;
    let mut cursor = 0;
    for line in src.split_inclusive('\n') {
        let line_start = cursor;
        cursor += line.len();
        let trimmed_end = line.trim_end_matches('\n');
        if start.is_none() {
            if trimmed_end.starts_with(&header_prefix) {
                start = Some(line_start);
            }
        } else if trimmed_end == ")" {
            return Some(src[start.unwrap()..cursor].trim_end_matches('\n'));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_predicate_block_extracts_named() {
        let src = "\
FindLog(out) = AND(
  X(a, b)
  Y(c)
)

CraftWood(in, out) = AND(
  Z(d)
)
";
        let block = find_predicate_block(src, "CraftWood").unwrap();
        assert_eq!(
            block,
            "CraftWood(in, out) = AND(\n  Z(d)\n)"
        );
    }

    #[test]
    fn find_predicate_block_returns_none_when_absent() {
        let src = "FindLog(out) = AND(\n  X(a)\n)\n";
        assert!(find_predicate_block(src, "Missing").is_none());
    }

    #[test]
    fn find_predicate_block_ignores_substring_matches() {
        let src = "\
FindLogger(x) = AND(
  Y(z)
)

FindLog(out) = AND(
  X(a)
)
";
        let block = find_predicate_block(src, "FindLog").unwrap();
        assert!(block.starts_with("FindLog(out)"));
    }
}
