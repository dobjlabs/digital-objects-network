//! `pexe inspect` subcommand handlers: read-only views of a plugin's
//! predicates, classes, and action graph. Each handler accepts a target
//! path that is either a `.pexe` archive or a source directory holding
//! `manifest.toml` + `plugin.rhai`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Result, anyhow};
use pod2::lang::PrettyPrint;
use pod2::middleware::{Hash, Predicate, PredicateOrWildcard};
use sdk::{Dependency, FieldWrites, ObjectIdentity, Pin, Sdk, SdkModule, manifest::Manifest};

use crate::{PluginSource, read_pexe_file, unpack};

/// Resolve a target path to its parsed manifest and plugin script.
/// Directories are read via `PluginSource::read`; anything else is
/// treated as a `.pexe` archive and unpacked.
fn load_target(path: &Path) -> Result<(Manifest, String)> {
    if path.is_dir() {
        let source = PluginSource::read(path)?;
        let manifest = source.parse_manifest()?;
        Ok((manifest, source.script))
    } else {
        let bytes = read_pexe_file(path)?;
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
        // Scan `batch_text` once to build a name -> block map so the
        // per-predicate lookups in the queue are O(1) instead of O(len).
        let block_index = index_predicate_blocks(&batch_text);
        let mut first = true;
        for idx in order {
            let Some(pred) = predicates.get(idx) else {
                continue;
            };
            let Some(block) = block_index.get(pred.name.as_str()) else {
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
    src.lines().find(|line| line.starts_with(&needle))
}

/// `pexe inspect plan`.
///
/// Mint one synthetic object per input declared by `action`, fabricate
/// a grounded state for them, and run the SDK's solver in mock mode
/// without proving. Prints three sections to stdout:
///
/// 1. **Header**: action name with input / output classes.
/// 2. **Solution breakdown**: the multi-pod solver's per-POD
///    utilization summary (statements, merkle proofs, etc.).
/// 3. **Statement dep graph**: per-POD list of statements in chain
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

/// Shared loaded state for `plan` and `prove_action`: the compiled
/// plugin, the named action's input/output class lists, the synthetic
/// grounded state ready to feed into an executor.
struct ActionRun {
    module: std::rc::Rc<SdkModule>,
    input_classes: Vec<String>,
    output_classes: Vec<String>,
    state: crate::fixtures::SyntheticState,
}

fn prepare_run(target: &Path, action_name: &str) -> Result<ActionRun> {
    let (manifest, script) = load_target(target)?;
    let module = load_sdk_module(&manifest, &script)?;
    let action = module
        .actions()
        .iter()
        .find(|a| a.name == action_name)
        .ok_or_else(|| anyhow!("no action named {action_name} in this plugin"))?;
    let input_classes: Vec<String> = action.total_inputs().map(|r| r.class.clone()).collect();
    let output_classes: Vec<String> = action.total_outputs().map(|r| r.class.clone()).collect();
    let minted = crate::fixtures::mint_action_inputs(&module, action)?;
    let state = crate::fixtures::build_synthetic_state(&minted)?;
    Ok(ActionRun {
        module,
        input_classes,
        output_classes,
        state,
    })
}

/// `pexe prove`. Same shape as `inspect plan` but with `mock=false`,
/// so the action is actually proved via the real plonky2 prover. This
/// is much slower than `plan` (minutes for actions with many PODs).
pub fn prove_action(target: &Path, action_name: &str) -> Result<()> {
    let run = prepare_run(target, action_name)?;

    println!("Prove: {}", action_name);
    println!("  Inputs ({}):", run.input_classes.len());
    for class in &run.input_classes {
        println!("    - {class}");
    }
    println!("  Outputs ({}):", run.output_classes.len());
    for class in &run.output_classes {
        println!("    - {class}");
    }
    println!();
    println!("Proving via real plonky2 backend. This may take several minutes.");
    println!();

    let executor = run
        .module
        .executor(false, run.state.grounding_witness.clone());
    let start = std::time::Instant::now();
    let outputs = executor
        .action(action_name, run.state.spendable)
        .map_err(|err| anyhow!("proving failed: {err}"))?;
    let elapsed = start.elapsed();

    println!();
    println!("Proved in {:.2}s", elapsed.as_secs_f64());
    println!("tx_final: {:#}", outputs.tx.ctx.commitment());
    println!("Output objects ({}):", outputs.objs.len());
    for (i, obj) in outputs.objs.iter().enumerate() {
        println!("  [{i}] commitment={:#}", obj.obj.commitment());
    }

    Ok(())
}

pub fn plan(target: &Path, action_name: &str, mode: PlanOutput) -> Result<()> {
    let run = prepare_run(target, action_name)?;
    let input_classes = run.input_classes;
    let output_classes = run.output_classes;
    let executor = run
        .module
        .executor(true, run.state.grounding_witness.clone());
    let plan = executor
        .plan_action(action_name, run.state.spendable)
        .map_err(|err| anyhow!("planning failed: {err}"))?;
    let aliases = build_alias_map(&run.module);

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
                .map(|dep| match dep {
                    AbstractDep::Internal(idx) => format!("[{idx}]"),
                    AbstractDep::External { pod, statement } => format!("ext{pod}:{statement}"),
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

fn statement_label(stmt: &pod2::middleware::Statement, aliases: &HashMap<Hash, String>) -> String {
    match stmt.predicate() {
        Predicate::Native(n) => format!("{n}"),
        Predicate::Custom(c) => format_custom_name(&c, aliases),
        Predicate::Intro(i) => i.name.clone(),
        Predicate::BatchSelf(idx) => format!("batch_self_{idx}"),
    }
}

/// Count each distinct custom predicate by occurrences in the plan's
/// statement list. Native and Intro statements are excluded: they're
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

    // Index statements once so each ReplaceValueWithEntry op resolves
    // its source in O(1) instead of scanning the statement list.
    let stmt_index: HashMap<&pod2::middleware::Statement, usize> = plan
        .statements
        .iter()
        .enumerate()
        .map(|(i, s)| (s, i))
        .collect();

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
        if let Some(&src_idx) = stmt_index.get(source_stmt) {
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
    let view = build_graph_view(plan, compressed);
    let output = plan.solved.solution();
    let mode_tag = if view.compressed {
        "compressed"
    } else {
        "full"
    };
    let suffix = if compressed { "" } else { "_full" };

    let mut out = String::new();
    out.push_str(&format!(
        "digraph plan_{}{suffix} {{\n",
        sanitize(action_name)
    ));
    out.push_str("  rankdir=TB;\n");
    out.push_str("  node [fontname=\"Helvetica\", shape=box, style=\"rounded,filled\", fillcolor=\"#f6f8fa\"];\n");
    out.push_str("  edge [fontname=\"Helvetica\", fontsize=10];\n");
    out.push_str("  concentrate=true;\n");
    out.push_str(&format!(
        "  label=\"{} ({mode_tag}) -- inputs: [{}], outputs: [{}]\";\n",
        action_name,
        input_classes.join(", "),
        output_classes.join(", "),
    ));
    out.push_str("  labelloc=t;\n\n");

    for (pod_idx, visible) in view.pod_visible.iter().enumerate() {
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
        for &s in visible {
            let label = statement_label(&plan.statements[s], aliases);
            out.push_str(&format!("    s{s} [label=\"[{s}] {label}\"];\n"));
        }
        out.push_str("  }\n");
    }
    out.push('\n');

    if !view.external_refs.is_empty() {
        out.push_str("  subgraph cluster_external {\n");
        out.push_str("    label=\"external pods\";\n");
        out.push_str("    style=dotted;\n");
        for (pod, stmt) in &view.external_refs {
            out.push_str(&format!(
                "    ext{pod}_{stmt} [label=\"ext{pod}:{stmt}\", shape=note, fillcolor=\"#fff5b7\"];\n",
            ));
        }
        out.push_str("  }\n\n");
    }

    for (&s, (internal, external)) in &view.edges {
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

/// Map a class or action name to a Mermaid-safe node id. Mermaid
/// flowchart ids accept `[A-Za-z0-9_]` only, but class/action names
/// also allow `-`. Escape both punctuation chars distinctly so that
/// `Foo_Bar` and `Foo-Bar` don't collapse onto the same node.
fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            c if c.is_ascii_alphanumeric() => out.push(c),
            '_' => out.push_str("_u"),
            '-' => out.push_str("_h"),
            _ => out.push('_'),
        }
    }
    out
}

/// (internal producer indices, external (pod, stmt) refs) for one
/// visible node after Native folding and rewrite resolution.
type EdgeSet = (BTreeSet<usize>, BTreeSet<(usize, usize)>);

/// Structural view of the dep graph, abstracted away from the output
/// format. Built once per plan + compression-mode and consumed by
/// either the DOT or Mermaid renderer.
struct GraphView {
    /// Per-POD list of statement indices visible in this view.
    pod_visible: Vec<Vec<usize>>,
    /// Per-visible-node, its inbound edge set.
    edges: BTreeMap<usize, EdgeSet>,
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

    let mut edges: BTreeMap<usize, EdgeSet> = BTreeMap::new();
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
    let mode_tag = if view.compressed {
        "compressed"
    } else {
        "full"
    };

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

    out.push('\n');
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

#[derive(Clone, Debug)]
pub enum GraphOutput {
    Dot,
    Mermaid,
    MermaidLink,
}

/// `pexe inspect graph`.
///
/// Emits the action/class relationship graph. Class nodes (boxes) sit
/// at the perimeter; action nodes (ellipses) are connected via `in` /
/// `out` / `mutate` edges. With `GraphOutput::Dot`, pipe to `dot -Tsvg`;
/// with `GraphOutput::Mermaid`, paste into a markdown renderer; with
/// `GraphOutput::MermaidLink`, opens directly in mermaid.live.
pub fn graph(target: &Path, mode: GraphOutput) -> Result<()> {
    let (manifest, script) = load_target(target)?;
    let module = load_sdk_module(&manifest, &script)?;

    match mode {
        GraphOutput::Dot => print!("{}", build_class_graph_dot(&module)),
        GraphOutput::Mermaid => print!("{}", build_class_graph_mermaid(&module)),
        GraphOutput::MermaidLink => {
            let source = build_class_graph_mermaid(&module);
            match mermaid_live_url(&source) {
                Ok(url) => println!("{url}"),
                Err(err) => {
                    eprintln!("failed to build mermaid.live URL: {err}");
                    print!("{source}");
                }
            }
        }
    }
    Ok(())
}

/// Per-action partition of class refs into pure-in, pure-out, and
/// mutate (intersection of inputs and outputs). Used by both the DOT
/// and Mermaid class-graph renderers.
fn action_edges(
    action: &sdk::ActionMeta,
) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    let inputs: BTreeSet<String> = action.local_inputs().map(|r| r.class.clone()).collect();
    let outputs: BTreeSet<String> = action.local_outputs().map(|r| r.class.clone()).collect();
    let mutates: BTreeSet<String> = inputs.intersection(&outputs).cloned().collect();
    let pure_in: BTreeSet<String> = inputs.difference(&mutates).cloned().collect();
    let pure_out: BTreeSet<String> = outputs.difference(&mutates).cloned().collect();
    (pure_in, pure_out, mutates)
}

fn build_class_graph_dot(module: &SdkModule) -> String {
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
        let (pure_in, pure_out, mutates) = action_edges(action);
        for class in &pure_in {
            out.push_str(&format!(
                "  \"class_{class}\" -> \"action_{action}\" [label=\"in\"];\n",
                action = action.name,
            ));
        }
        for class in &pure_out {
            out.push_str(&format!(
                "  \"action_{}\" -> \"class_{class}\" [label=\"out\"];\n",
                action.name,
            ));
        }
        for class in &mutates {
            out.push_str(&format!(
                "  \"action_{}\" -> \"class_{class}\" [label=\"mutate\", dir=both, color=darkorange];\n",
                action.name,
            ));
        }
    }

    out.push_str("}\n");
    out
}

fn build_class_graph_mermaid(module: &SdkModule) -> String {
    let mut out = String::new();
    out.push_str("flowchart LR\n");

    for class in module.classes() {
        out.push_str(&format!(
            "  class_{}[\"{}\"]\n",
            sanitize(&class.name),
            escape_mermaid(&class.name),
        ));
    }
    for action in module.actions() {
        out.push_str(&format!(
            "  action_{}([\"{}\"])\n",
            sanitize(&action.name),
            escape_mermaid(&action.name),
        ));
    }
    out.push('\n');

    for action in module.actions() {
        let action_id = sanitize(&action.name);
        let (pure_in, pure_out, mutates) = action_edges(action);
        for class in &pure_in {
            out.push_str(&format!(
                "  class_{} -->|in| action_{action_id}\n",
                sanitize(class),
            ));
        }
        for class in &pure_out {
            out.push_str(&format!(
                "  action_{action_id} -->|out| class_{}\n",
                sanitize(class),
            ));
        }
        for class in &mutates {
            out.push_str(&format!(
                "  action_{action_id} <-->|mutate| class_{}\n",
                sanitize(class),
            ));
        }
    }

    if !module.classes().is_empty() {
        out.push('\n');
        out.push_str("  classDef cls fill:#add8e6,stroke:#333,color:#000;\n");
        let ids: Vec<String> = module
            .classes()
            .iter()
            .map(|c| format!("class_{}", sanitize(&c.name)))
            .collect();
        out.push_str(&format!("  class {} cls;\n", ids.join(",")));
    }

    out
}

/// `pexe inspect classes`.
///
/// Render each class's state-space signature in "Notation A": a
/// pod2-typed record listing application fields with literal narrowing
/// where possible and a crypto appendix for VDF/PoW-derived fields.
pub fn classes(target: &Path, class_filter: Option<&str>) -> Result<()> {
    let (manifest, script) = load_target(target)?;
    let module = load_sdk_module(&manifest, &script)?;
    let signatures = class_signatures(&module);

    let empty = ClassSignature::default();
    let mut first = true;
    let mut matched = false;
    for class in module.classes() {
        if let Some(name) = class_filter
            && class.name != name
        {
            continue;
        }
        matched = true;
        if !first {
            println!();
        }
        first = false;
        let signature = signatures.get(&class.name).unwrap_or(&empty);
        println!("{}", render_signature(&class.name, signature));
    }
    if let Some(name) = class_filter
        && !matched
    {
        return Err(anyhow!("no class named {name} in this plugin"));
    }
    Ok(())
}

/// A class's shape, folded over every action that creates or mutates an
/// instance. Only written values count: what an action *requires* of a
/// state it consumes says nothing about the shape of the class.
///
/// Creating and mutating actions are folded separately, because a value
/// only a mutation writes is one no fresh instance of the class holds.
fn class_signatures(module: &SdkModule) -> BTreeMap<String, ClassSignature> {
    let mut out: BTreeMap<String, ClassSignature> = BTreeMap::new();
    for action in module.actions() {
        for obj in action.total_outputs() {
            let sig = out.entry(obj.class.clone()).or_default();
            sig.identity.absorb(obj.identity());
            for (field, writes) in obj.field_writes() {
                sig.fields
                    .entry(field.to_string())
                    .or_default()
                    .ever
                    .absorb(writes);
            }
        }
        for obj in action.total_created() {
            let sig = out.entry(obj.class.clone()).or_default();
            for (field, writes) in obj.field_writes() {
                sig.fields
                    .entry(field.to_string())
                    .or_default()
                    .at_mint
                    .absorb(writes);
            }
        }
    }
    out
}

/// One class's fields, each carrying every value any producer writes it
/// with.
#[derive(Default)]
struct ClassSignature {
    fields: BTreeMap<String, ClassField>,
    identity: ObjectIdentity,
}

/// One field of a class, split by whether a value is reachable at mint.
#[derive(Default)]
struct ClassField {
    /// Written by an action that creates the object.
    at_mint: FieldWrites,
    /// Written by any action, creating or mutating.
    ever: FieldWrites,
}

fn render_signature(name: &str, sig: &ClassSignature) -> String {
    let mut out = format!("class {name} {{\n");
    let width = sig.fields.keys().map(|f| f.len()).max().unwrap_or(0);
    for (field, summary) in &sig.fields {
        out.push_str(&format!(
            "  {field:width$}  {}\n",
            render_field_value(summary)
        ));
    }
    if sig.identity.is_constrained() {
        out.push_str("  // identity:");
        if sig.identity.proof_of_work {
            out.push_str(" PoW (lt_eq_u256)");
        }
        if sig.identity.vdf {
            out.push_str(" VDF");
        }
        out.push('\n');
    }
    out.push('}');
    out
}

fn render_field_value(field: &ClassField) -> String {
    let mint = &field.at_mint.values;
    let later: BTreeSet<&Pin> = field.ever.values.difference(mint).collect();

    let kind = match kind_of(field.ever.values.iter()) {
        Some(kind) => kind,
        None if field.ever.from_vdf => return "Raw  // VDF-derived".to_string(),
        // Nothing any producer writes fixes the value, so a prover
        // supplies it.
        None => return "Raw  // witness".to_string(),
    };

    let mut clauses: Vec<String> = Vec::new();
    if !mint.is_empty() {
        clauses.push(format!("at mint: {}", join_pins(mint.iter())));
    }
    if !later.is_empty() {
        clauses.push(format!("updated to {}", join_pins(later.into_iter())));
    }
    format!("{kind}  // {}", clauses.join("; "))
}

/// The type column, or `None` when no producer writes a literal.
fn kind_of<'a>(values: impl Iterator<Item = &'a Pin>) -> Option<&'static str> {
    let (mut text, mut int) = (false, false);
    for v in values {
        match v {
            Pin::Text(_) => text = true,
            Pin::Int(_) => int = true,
        }
    }
    match (text, int) {
        (false, false) => None,
        (true, false) => Some("Str"),
        (false, true) => Some("Int"),
        // A class whose schema is fixed should not hold both; rendering
        // the union says so without guessing which producer is wrong.
        (true, true) => Some("Str | Int"),
    }
}

fn join_pins<'a>(values: impl Iterator<Item = &'a Pin>) -> String {
    let (texts, ints): (Vec<&Pin>, Vec<&Pin>) = values.partition(|p| matches!(p, Pin::Text(_)));
    texts
        .into_iter()
        .chain(ints)
        .map(Pin::to_string)
        .collect::<Vec<_>>()
        .join(" | ")
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
        match start {
            None => {
                if trimmed_end.starts_with(&header_prefix) {
                    start = Some(line_start);
                }
            }
            Some(s) if trimmed_end == ")" => {
                return Some(src[s..cursor].trim_end_matches('\n'));
            }
            _ => {}
        }
    }
    None
}

/// Single-pass index of every top-level predicate block in `src`,
/// keyed by predicate name. Used by [`print_middleware`] so its
/// per-predicate lookups don't each rescan the whole batch text.
fn index_predicate_blocks(src: &str) -> HashMap<&str, &str> {
    let mut out: HashMap<&str, &str> = HashMap::new();
    let mut block_start: Option<usize> = None;
    let mut block_name: Option<&str> = None;
    let mut cursor = 0;
    for line in src.split_inclusive('\n') {
        let line_start = cursor;
        cursor += line.len();
        let trimmed_end = line.trim_end_matches('\n');
        if block_start.is_none() {
            if let Some(paren) = trimmed_end.find('(') {
                let name = &trimmed_end[..paren];
                if !name.is_empty()
                    && name
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                {
                    block_start = Some(line_start);
                    block_name = Some(&src[line_start..line_start + paren]);
                }
            }
        } else if trimmed_end == ")" {
            if let (Some(s), Some(n)) = (block_start, block_name) {
                let block = src[s..cursor].trim_end_matches('\n');
                out.insert(n, block);
            }
            block_start = None;
            block_name = None;
        }
    }
    out
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
        assert_eq!(block, "CraftWood(in, out) = AND(\n  Z(d)\n)");
    }

    #[test]
    fn find_predicate_block_returns_none_when_absent() {
        let src = "FindLog(out) = AND(\n  X(a)\n)\n";
        assert!(find_predicate_block(src, "Missing").is_none());
    }

    #[test]
    fn sanitize_distinguishes_underscore_and_hyphen() {
        assert_eq!(sanitize("Foo_Bar"), "Foo_uBar");
        assert_eq!(sanitize("Foo-Bar"), "Foo_hBar");
        assert_ne!(sanitize("Foo_Bar"), sanitize("Foo-Bar"));
        assert_eq!(sanitize("PlainName"), "PlainName");
    }

    fn written(values: &[Pin], from_vdf: bool) -> FieldWrites {
        FieldWrites {
            values: values.iter().cloned().collect(),
            from_vdf,
        }
    }

    /// A value only a mutation writes is not one a fresh instance holds,
    /// so the two are labelled apart.
    #[test]
    fn render_signature_separates_mint_from_later_values() {
        let mut sig = ClassSignature::default();
        sig.fields.insert(
            "durability".to_string(),
            ClassField {
                at_mint: written(&[Pin::Int(100)], false),
                ever: written(&[Pin::Int(100)], false),
            },
        );
        sig.fields.insert(
            "authorized".to_string(),
            ClassField {
                at_mint: written(&[Pin::Int(0)], false),
                ever: written(&[Pin::Int(0), Pin::Int(1)], false),
            },
        );
        sig.fields.insert(
            "revealed".to_string(),
            ClassField {
                at_mint: written(&[], false),
                ever: written(&[Pin::Int(1)], false),
            },
        );
        sig.fields.insert(
            "work".to_string(),
            ClassField {
                at_mint: written(&[], true),
                ever: written(&[], true),
            },
        );
        sig.identity.vdf = true;

        let rendered = render_signature("WoodPick", &sig);
        assert!(rendered.contains("Int  // at mint: 100"), "{rendered}");
        assert!(
            rendered.contains("Int  // at mint: 0; updated to 1"),
            "{rendered}"
        );
        assert!(rendered.contains("Int  // updated to 1"), "{rendered}");
        assert!(rendered.contains("Raw  // VDF-derived"), "{rendered}");
        assert!(rendered.contains("// identity: VDF"), "{rendered}");
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
