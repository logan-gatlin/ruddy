//! Source-level abbreviations. This is deliberately separate from `Scheme`'s
//! annotation-only Display: a declaration cannot be inserted after `let f:`.
use super::*;

/// A complete presentation: ordinary type declarations followed by an annotation.
/// Callers inserting the annotation into a declaration must put `definitions`
/// before that declaration (or use [`Self::declaration`]).
#[derive(Debug)]
pub struct TypePresentation {
    pub definitions: Vec<String>,
    pub annotation: String,
}

impl TypePresentation {
    /// `aliases` must contain only names accessible in the destination scope.
    /// All supplied names are reserved, including aliases ineligible for reuse.
    /// Reuse is conservative: unconstrained, fully parameterized structural
    /// definitions with the same shape; no unfolding or speculative equivalence.
    pub fn new<'a>(
        scheme: &Scheme,
        aliases: impl IntoIterator<Item = (&'a str, &'a Scheme)>,
    ) -> Self {
        let aliases: Vec<_> = aliases.into_iter().collect();
        let plan = PresencePlan::new(scheme);
        let compact = Self::with_plan(scheme, &aliases, &plan);
        if plan.bounds.is_empty() {
            return compact;
        }
        // An inline bound cannot be moved into an ordinary alias definition.
        // Compare against keeping its parameter explicit and abbreviating the
        // surrounding structure; count helper definitions in both choices.
        let explicit = Self::with_plan(scheme, &aliases, &PresencePlan::explicit(scheme));
        if cost(&explicit.definitions, &explicit.annotation)
            < cost(&compact.definitions, &compact.annotation)
        {
            explicit
        } else {
            compact
        }
    }

    fn with_plan(scheme: &Scheme, aliases: &[(&str, &Scheme)], plan: &PresencePlan) -> Self {
        let names = scheme_names(scheme);
        let mut replacements = HashMap::new();
        let mut definitions = Vec::new();
        let mut reserved: HashSet<String> =
            aliases.iter().map(|(name, _)| name.to_string()).collect();
        // Bounded optimization, not bounded display. Large or recovery types
        // always fall back to the complete explicit-stack semantic formatter.
        let Some(all) = scan(scheme.body(), 4096) else {
            return Self {
                definitions,
                annotation: scheme.to_string(),
            };
        };
        for ty in &all.types {
            if let Ty::Named { name, .. } = ty {
                reserved.insert(name.to_string());
            }
        }
        let mut groups: Vec<Vec<Template<'_>>> = Vec::new();
        let mut by_shape = HashMap::new();
        for ty in all.types {
            if !matches!(
                ty,
                Ty::Struct(_) | Ty::Sum(_) | Ty::Array(_) | Ty::Arrow(..) | Ty::Mut(..)
            ) {
                continue;
            }
            if let Some(template) = Template::new(ty, scheme.count()) {
                if template
                    .bounds
                    .iter()
                    .any(|index| scheme.is_existential(*index) || plan.bounds.contains_key(index))
                {
                    continue;
                }
                let next = groups.len();
                let index = *by_shape
                    .entry((template.shape.clone(), template.effects.clone()))
                    .or_insert(next);
                if index == groups.len() {
                    groups.push(Vec::new());
                }
                groups[index].push(template);
            }
        }
        let known: Vec<_> = aliases
            .iter()
            .filter_map(|(name, alias)| {
                if !alias.formula().is_true() || !alias.existentials().is_empty() {
                    return None;
                }
                let template = Template::new(alias.body(), alias.count())?;
                // Phantom parameters cannot be reconstructed from a matched body.
                (template.bounds.len() == alias.count() as usize).then_some((*name, template))
            })
            .collect();
        let mut annotation = render_scheme(scheme, &names, &replacements, plan);
        let body = Render {
            ty: scheme.body(),
            names: &names,
            replacements: &replacements,
            formula: None,
            bounds: &plan.bounds,
        }
        .to_string();
        let constraints = annotation[body.len()..].to_string();
        // Choose by the entire output size, not just the shortened annotation.
        // At most eight new declarations keeps the search bounded as well.
        for _ in 0..8 {
            let old_cost = cost(&definitions, &annotation);
            let mut best_cost = old_cost;
            let mut best = None;
            for (group_index, group) in groups.iter().enumerate() {
                let first = &group[0];
                let base = match first.ty {
                    Ty::Struct(_) => "InferredRecord",
                    Ty::Sum(_) => "InferredChoice",
                    _ => "InferredType",
                };
                let mut fresh = base.to_string();
                let mut suffix = 2;
                while reserved.contains(&fresh) {
                    fresh = format!("{base}{suffix}");
                    suffix += 1;
                }
                let mut choices: Vec<(&str, Option<&Template<'_>>)> = known
                    .iter()
                    .filter(|(_, template)| {
                        template.shape == first.shape && template.effects == first.effects
                    })
                    .map(|(name, template)| (*name, Some(template)))
                    .collect();
                if group.len() > 1 {
                    choices.push((&fresh, None));
                }
                for (name, existing) in choices {
                    let mut trial = replacements.clone();
                    for occurrence in group {
                        let mut args = vec![String::new(); first.bounds.len()];
                        for (position, index) in occurrence.bounds.iter().enumerate() {
                            let slot = existing.map_or(position, |t| t.bounds[position] as usize);
                            args[slot] = match names.get(*index as usize) {
                                Some(name) if name.is_empty() => "_".to_string(),
                                _ => bound_name(*index, Some(&names)),
                            };
                            if let Some(effects) = occurrence.rows.get(index) {
                                args[slot] = if *effects {
                                    format!("(..{})", args[slot])
                                } else {
                                    format!("{{ ..{} }}", args[slot])
                                };
                            }
                        }
                        let application = if args.is_empty() {
                            name.to_string()
                        } else {
                            format!("{name} {}", args.join(" "))
                        };
                        trial.insert(occurrence.ty as *const Ty as usize, application);
                    }
                    let mut trial_definitions = definitions.clone();
                    if existing.is_none() {
                        let params: String = (0..first.bounds.len())
                            .map(|i| format!(" {}", name_at(i as u32)))
                            .collect();
                        trial_definitions.push(format!("type {name}{params} = {}", first.shape));
                    }
                    let trial_annotation = Render {
                        ty: scheme.body(),
                        names: &names,
                        replacements: &trial,
                        formula: None,
                        bounds: &plan.bounds,
                    }
                    .to_string()
                        + &constraints;
                    let trial_cost = cost(&trial_definitions, &trial_annotation);
                    if trial_cost < best_cost {
                        best_cost = trial_cost;
                        best = Some((
                            trial_cost,
                            group_index,
                            name.to_string(),
                            trial,
                            trial_definitions,
                            trial_annotation,
                        ));
                    }
                }
            }
            let Some((_, index, name, trial, defs, text)) = best else {
                break;
            };
            replacements = trial;
            definitions = defs;
            annotation = text;
            reserved.insert(name);
            groups.remove(index);
        }
        Self {
            definitions,
            annotation,
        }
    }

    /// Render definitions *before* a declaration prefix such as `let f: `.
    pub fn declaration(&self, prefix: &str) -> String {
        let mut text = String::new();
        for definition in &self.definitions {
            text.push_str(definition);
            text.push('\n');
        }
        text.push_str(prefix);
        text.push_str(&self.annotation);
        text
    }
}

impl fmt::Display for TypePresentation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.declaration(""))
    }
}

fn cost(definitions: &[String], annotation: &str) -> usize {
    annotation.chars().count()
        + definitions
            .iter()
            .map(|s| s.chars().count() + 1)
            .sum::<usize>()
}

struct Render<'a> {
    ty: &'a Ty,
    names: &'a [String],
    replacements: &'a HashMap<usize, String>,
    formula: Option<&'a Formula>,
    bounds: &'a HashMap<u32, u32>,
}

impl fmt::Display for Render<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_semantic_replacing(
            f,
            SemanticRoot::Ty(self.ty),
            Some(self.names),
            self.replacements,
            self.bounds,
        )?;
        if let Some(formula) = self.formula {
            format_scheme_constraints(f, formula, self.names)?;
        }
        Ok(())
    }
}

fn render_scheme(
    scheme: &Scheme,
    names: &[String],
    replacements: &HashMap<usize, String>,
    plan: &PresencePlan,
) -> String {
    Render {
        ty: scheme.body(),
        names,
        replacements,
        formula: Some(&plan.formula),
        bounds: &plan.bounds,
    }
    .to_string()
}

struct Template<'a> {
    ty: &'a Ty,
    shape: String,
    bounds: Vec<u32>,
    rows: HashMap<u32, bool>,
    // Effect labels may print without their structural interface identity.
    // Equal rendered text alone is therefore not enough for matching them.
    effects: Vec<String>,
}

impl<'a> Template<'a> {
    fn new(ty: &'a Ty, count: u32) -> Option<Self> {
        let scan = scan(ty, 512)?;
        if !scan.safe {
            return None;
        }
        let mut bounds = Vec::new();
        let mut names = vec![String::new(); count as usize];
        for index in scan.bounds {
            let slot = names.get_mut(index as usize)?;
            if slot.is_empty() {
                *slot = name_at(bounds.len() as u32);
                bounds.push(index);
            }
        }
        let shape = Render {
            ty,
            names: &names,
            replacements: &HashMap::new(),
            formula: None,
            bounds: &HashMap::new(),
        }
        .to_string();
        Some(Self {
            ty,
            shape,
            bounds,
            rows: scan.rows,
            effects: scan.effects,
        })
    }
}

struct Scan<'a> {
    types: Vec<&'a Ty>,
    bounds: Vec<u32>,
    presences: Vec<u32>,
    presence_arguments: HashSet<u32>,
    safe: bool,
    rows: HashMap<u32, bool>,
    effects: Vec<String>,
}

/// Count occurrences, not distinct Arc addresses: two paths to the same node
/// still express sharing. The same walk supplies template parameters in order.
fn scan(ty: &Ty, limit: usize) -> Option<Scan<'_>> {
    enum Job<'a> {
        Ty(&'a Ty),
        Row(&'a Row, bool),
        Rest(&'a Rest, bool),
        Presence(&'a Presence),
    }
    let mut out = Scan {
        types: Vec::new(),
        bounds: Vec::new(),
        presences: Vec::new(),
        presence_arguments: HashSet::new(),
        safe: true,
        rows: HashMap::new(),
        effects: Vec::new(),
    };
    let mut work = vec![Job::Ty(ty)];
    let mut visited = 0;
    while let Some(job) = work.pop() {
        visited += 1;
        if visited > limit {
            return None;
        }
        match job {
            Job::Ty(ty) => {
                out.types.push(ty);
                match ty {
                    Ty::Bound(index) => out.bounds.push(*index),
                    Ty::Presence(p) => {
                        if let Presence::Bound(index) = p {
                            out.presence_arguments.insert(*index);
                        }
                        work.push(Job::Presence(p));
                    }
                    Ty::Struct(row) | Ty::Sum(row) => work.push(Job::Row(row, false)),
                    Ty::Arrow(from, to, effects) => {
                        work.push(Job::Row(effects, true));
                        work.push(Job::Ty(to));
                        work.push(Job::Ty(from));
                    }
                    Ty::Array(inner) | Ty::Mirror(inner) | Ty::TypeInfo(inner) => {
                        work.push(Job::Ty(inner))
                    }
                    Ty::Mut(region, inner) => {
                        work.push(Job::Ty(inner));
                        work.push(Job::Ty(region));
                    }
                    Ty::Package(inner) | Ty::Hidden { body: inner, .. } => {
                        out.safe = false;
                        work.push(Job::Ty(inner));
                    }
                    Ty::Named { args, .. } => {
                        // A textual name alone is not a semantic identity. Do
                        // not alpha-match through names from different scopes.
                        out.safe = false;
                        work.extend(args.iter().rev().map(|arg| Job::Ty(arg)));
                    }
                    Ty::Var(_) | Ty::Rigid { .. } | Ty::HiddenVar { .. } | Ty::Undecided => {
                        out.safe = false
                    }
                    _ => {}
                }
            }
            Job::Row(row, effects) => {
                if effects {
                    out.effects.extend(row.labels.keys().cloned());
                }
                work.push(Job::Rest(&row.rest, effects));
                for field in row.labels.values().rev() {
                    work.push(Job::Ty(&field.ty));
                    work.push(Job::Presence(&field.presence));
                }
            }
            Job::Rest(rest, effects) => match rest {
                Rest::Bound(index) => {
                    out.bounds.push(*index);
                    out.rows.insert(*index, effects);
                }
                Rest::More(row) => {
                    // Flattening can mask entries. Do not lift parameters
                    // whose occurrences would disappear from the definition.
                    out.safe = false;
                    work.push(Job::Row(row, effects));
                }
                Rest::Closed => {}
                _ => out.safe = false,
            },
            Job::Presence(p) => match p {
                Presence::Bound(index) => {
                    out.bounds.push(*index);
                    out.presences.push(*index);
                }
                Presence::Present => {}
                _ => out.safe = false,
            },
        }
    }
    Some(out)
}

pub(super) fn anonymous_presences(scheme: &Scheme) -> Vec<u32> {
    let scan = scan(scheme.body(), usize::MAX).expect("unbounded iterative walk");
    let mut counts = HashMap::<u32, usize>::new();
    for index in scan.presences {
        *counts.entry(index).or_default() += 1;
    }
    let mut atoms = Vec::new();
    scheme.formula().atoms(&mut atoms);
    for atom in atoms {
        if let Atom::Bound(index) = atom {
            counts.remove(&index);
        }
    }
    counts
        .into_iter()
        .filter_map(|(index, count)| {
            (count == 1 && index < scheme.presences() && !scheme.is_existential(index))
                .then_some(index)
        })
        .collect()
}

/// Inline only an independent, universally quantified presence whose entire
/// relationship is one implication to a named presence. Keeping every shared
/// source, every ownership boundary, and every other constraint explicit makes
/// the shorthand an alpha-renaming plus relocation of that same implication.
pub(super) struct PresencePlan {
    pub bounds: HashMap<u32, u32>,
    pub formula: Formula,
}

impl PresencePlan {
    fn explicit(scheme: &Scheme) -> Self {
        Self {
            bounds: HashMap::new(),
            formula: scheme.formula().clone(),
        }
    }

    pub fn new(scheme: &Scheme) -> Self {
        let mut plan = Self::explicit(scheme);
        if scheme.presences() == 0 || scheme.formula().is_true() {
            return plan;
        }
        let Some(scan) = scan(scheme.body(), 4096) else {
            return plan;
        };
        // Alias extraction also rejects absent entries and row extensions, but
        // those are ordinary inference artifacts that the semantic formatter
        // flattens. Only count an inline source if its mark will really print.
        let Some(visible) = visible_presence_marks(scheme.body()) else {
            return plan;
        };
        let Some(scopes) = presence_constraint_scopes(
            scheme.formula(),
            matches!(scheme.body().as_ref(), Ty::Package(_)),
        ) else {
            return plan;
        };
        let mut occurrences = HashMap::<u32, usize>::new();
        for index in scan.presences {
            *occurrences.entry(index).or_default() += 1;
        }
        let eligible = |index: u32| {
            index < scheme.presences()
                && occurrences.get(&index) == Some(&1)
                && !scheme.is_existential(index)
                && !scan.presence_arguments.contains(&index)
                && visible.contains(&index)
        };
        let groups: Vec<_> = scopes
            .into_iter()
            .flat_map(|(owner, parts)| {
                inference::sat::presentation_groups(&Formula::all(parts))
                    .into_iter()
                    .map(move |group| (owner, group))
            })
            .collect();
        // A source used in two different ownership scopes still represents
        // one presence. Count all constraints before choosing any shorthand.
        let mut constraints = HashMap::<Atom, usize>::new();
        for (_, group) in &groups {
            let mut conjuncts = Vec::new();
            formula_conjuncts(&group.cnf, &mut conjuncts);
            for part in conjuncts {
                let mut atoms = Vec::new();
                part.atoms(&mut atoms);
                for atom in atoms {
                    *constraints.entry(atom).or_default() += 1;
                }
            }
        }
        let mut remainder = Vec::new();
        for (owner, group) in groups {
            let mut conjuncts = Vec::new();
            formula_conjuncts(&group.cnf, &mut conjuncts);
            let mut changed = false;
            let mut kept = Vec::new();
            for part in conjuncts {
                if let Some((Atom::Bound(source), Atom::Bound(target))) = atomic_implication(part)
                    && source != target
                    && eligible(source)
                    && constraints.get(&Atom::Bound(source)) == Some(&1)
                    && target < scheme.presences()
                    && visible.contains(&target)
                {
                    plan.bounds.insert(source, target);
                    changed = true;
                } else {
                    kept.push(part.clone());
                }
            }
            let retained = if changed {
                Formula::all(kept)
            } else {
                group.relationships
            };
            if retained.is_true() {
                continue;
            }
            remainder.push(match owner {
                Some(owner) => Formula::owned(owner, retained),
                None => retained,
            });
        }
        if !plan.bounds.is_empty() {
            plan.formula = Formula::all(remainder);
        }
        plan
    }
}

/// Follow the semantic formatter's visible rows, rather than counting fields
/// that an outer absent label masks. An outer package is transparent to the
/// existing formatter; nested packages and opaque/recovery types stay explicit.
fn visible_presence_marks(ty: &Ty) -> Option<HashSet<u32>> {
    enum Job<'a> {
        Ty(&'a Ty),
        Row(&'a Row),
    }
    let mut visible = HashSet::new();
    let root = match ty {
        Ty::Package(inner) => inner,
        _ => ty,
    };
    let mut work = vec![Job::Ty(root)];
    while let Some(job) = work.pop() {
        match job {
            Job::Ty(ty) => match ty {
                Ty::Struct(row) | Ty::Sum(row) => work.push(Job::Row(row)),
                Ty::Arrow(from, to, effects) => {
                    work.push(Job::Row(effects));
                    work.push(Job::Ty(to));
                    work.push(Job::Ty(from));
                }
                Ty::Array(inner) | Ty::Mirror(inner) | Ty::TypeInfo(inner) => {
                    work.push(Job::Ty(inner));
                }
                Ty::Mut(region, inner) => {
                    work.push(Job::Ty(region));
                    work.push(Job::Ty(inner));
                }
                Ty::Package(_)
                | Ty::Hidden { .. }
                | Ty::Named { .. }
                | Ty::Var(_)
                | Ty::Rigid { .. }
                | Ty::HiddenVar { .. }
                | Ty::Undecided => return None,
                _ => {}
            },
            Job::Row(row) => {
                let (fields, _) = flattened_row(row);
                for (_, field) in fields {
                    if let Presence::Bound(index) = field.presence {
                        visible.insert(index);
                    }
                    work.push(Job::Ty(&field.ty));
                }
            }
        }
    }
    Some(visible)
}

/// Keep root-package guarantees separate from ordinary requirements while
/// simplifying them. More deeply nested ownership keeps the explicit fallback.
fn presence_constraint_scopes(
    formula: &Formula,
    root_package: bool,
) -> Option<Vec<(Option<u32>, Vec<Formula>)>> {
    let mut scopes: Vec<(Option<u32>, Vec<Formula>)> = Vec::new();
    let mut work = vec![(None, formula)];
    while let Some((owner, part)) = work.pop() {
        match part {
            Formula::And(left, right) => {
                work.push((owner, right));
                work.push((owner, left));
            }
            Formula::Owned(0, inner) if root_package => work.push((Some(0), inner)),
            Formula::Owned(..) => return None,
            part => {
                let mut nested = vec![part];
                while let Some(inner) = nested.pop() {
                    match inner {
                        Formula::Owned(..) => return None,
                        Formula::Not(inner) => nested.push(inner),
                        Formula::And(left, right)
                        | Formula::Or(left, right)
                        | Formula::Iff(left, right)
                        | Formula::Xor(left, right) => {
                            nested.push(left);
                            nested.push(right);
                        }
                        _ => {}
                    }
                }
                if let Some((_, parts)) = scopes.iter_mut().find(|(scope, _)| *scope == owner) {
                    parts.push(part.clone());
                } else {
                    scopes.push((owner, vec![part.clone()]));
                }
            }
        }
    }
    Some(scopes)
}
