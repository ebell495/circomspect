use log::{debug, trace};
use std::collections::{HashMap, HashSet};

use program_structure::cfg::Cfg;
use program_structure::intermediate_representation::variable_meta::VariableMeta;
use program_structure::intermediate_representation::AssignOp;
use program_structure::ir::variable_meta::VariableUse;
use program_structure::ir::{Statement, VariableName};

/// This analysis computes the transitive closure of the constraint relation.
/// (Note that the resulting relation will be symmetric, but not reflexive in
/// general.)
#[derive(Clone, Default)]
pub struct ConstraintAnalysis {
    constraint_map: HashMap<VariableName, HashSet<VariableName>>,
    declarations: HashMap<VariableName, VariableUse>,
    definitions: HashMap<VariableName, VariableUse>,
}

impl ConstraintAnalysis {
    fn new() -> ConstraintAnalysis {
        ConstraintAnalysis::default()
    }

    /// Add the variable use corresponding to the definition of the variable.
    fn add_definition(&mut self, var: &VariableUse) {
        // TODO: Since we don't version components and signals, we may end up
        // overwriting component initializations here. For example, in the
        // following case the component initialization will be clobbered.
        //
        //   component c[2];
        //   ...
        //   c[0].in[0] <== 0;
        //   c[1].in[1] <== 1;
        //
        // The constraint map should probably track VariableAccesses rather
        // than VariableNames.
        self.definitions.insert(var.name().clone(), var.clone());
    }

    /// Get the variable use corresponding to the definition of the variable.
    pub fn get_definition(&self, var: &VariableName) -> Option<VariableUse> {
        self.definitions.get(var).cloned()
    }

    pub fn definitions(&self) -> impl Iterator<Item = &VariableUse> {
        self.definitions.values()
    }

    /// Add the variable use corresponding to the declaration of the variable.
    fn add_declaration(&mut self, var: &VariableUse) {
        self.declarations.insert(var.name().clone(), var.clone());
    }

    /// Get the variable use corresponding to the declaration of the variable.
    pub fn get_declaration(&self, var: &VariableName) -> Option<VariableUse> {
        self.declarations.get(var).cloned()
    }

    pub fn declarations(&self) -> impl Iterator<Item = &VariableUse> {
        self.declarations.values()
    }

    /// Add a constraint from source to sink.
    fn add_constraint_step(&mut self, source: &VariableName, sink: &VariableName) {
        let sinks = self.constraint_map.entry(source.clone()).or_default();
        sinks.insert(sink.clone());
    }

    /// Returns variables constrained in a single step by `source`.
    pub fn single_step_constraint(&self, source: &VariableName) -> HashSet<VariableName> {
        self.constraint_map.get(source).cloned().unwrap_or_default()
    }

    /// Returns variables constrained in one or more steps by `source`.
    pub fn multi_step_constraint(&self, source: &VariableName) -> HashSet<VariableName> {
        let mut result = HashSet::new();
        let mut update = self.single_step_constraint(source);
        while !update.is_subset(&result) {
            result.extend(update.iter().cloned());
            update = update.iter().flat_map(|source| self.single_step_constraint(source)).collect();
        }
        result
    }

    /// Returns true if the source constrains any of the sinks.
    pub fn constrains_any(&self, source: &VariableName, sinks: &HashSet<VariableName>) -> bool {
        self.multi_step_constraint(source).iter().any(|sink| sinks.contains(sink))
    }

    /// Returns the set of variables occurring in a constraint together with at
    /// least one other variable.
    pub fn constrained_variables(&self) -> HashSet<VariableName> {
        self.constraint_map.keys().cloned().collect::<HashSet<_>>()
    }
}

pub fn run_constraint_analysis(cfg: &Cfg) -> ConstraintAnalysis {
    debug!("running constraint analysis pass");
    let mut result = ConstraintAnalysis::new();

    use AssignOp::*;
    use Statement::*;
    for basic_block in cfg.iter() {
        for stmt in basic_block.iter() {
            trace!("visiting statement `{stmt:?}`");
            // Add definitions to the result.
            for var in stmt.variables_written() {
                result.add_definition(var);
            }
            match stmt {
                Declaration { meta, names, .. } => {
                    // Add declarations to the result.
                    for sink in names {
                        result.add_declaration(&VariableUse::new(meta, sink, &Vec::new()));
                    }
                }
                ConstraintEquality { .. } | Substitution { op: AssignConstraintSignal, .. } => {
                    for source in stmt.variables_used() {
                        for sink in stmt.variables_used() {
                            if source.name() != sink.name() {
                                trace!(
                                    "adding constraint step with source `{:?}` and sink `{:?}`",
                                    source.name(),
                                    sink.name()
                                );
                                result.add_constraint_step(source.name(), sink.name());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use parser::parse_definition;
    use program_structure::cfg::IntoCfg;
    use program_structure::constants::Curve;
    use program_structure::report::ReportCollection;

    use super::*;

    #[test]
    fn test_single_step_constraint() {
        let src = r#"
            template T(n) {
                signal input in;
                signal output out;
                signal tmp;

                tmp <== 2 * in;
                out <== in * in;

            }
        "#;
        let sources = [
            VariableName::from_name("in"),
            VariableName::from_name("out"),
            VariableName::from_name("tmp"),
        ];
        let sinks = [2, 1, 1];
        validate_constraints(src, &sources, &sinks);

        let src = r#"
            template T(n) {
                signal input in;
                signal output out;
                signal tmp;

                tmp === 2 * in;
                out <== in * in;

            }
        "#;
        let sources = [
            VariableName::from_name("in"),
            VariableName::from_name("out"),
            VariableName::from_name("tmp"),
        ];
        let sinks = [2, 1, 1];
        validate_constraints(src, &sources, &sinks);
    }

    fn validate_constraints(src: &str, sources: &[VariableName], sinks: &[usize]) {
        // Build CFG.
        let mut reports = ReportCollection::new();
        let cfg = parse_definition(src)
            .unwrap()
            .into_cfg(&Curve::default(), &mut reports)
            .unwrap()
            .into_ssa()
            .unwrap();
        assert!(reports.is_empty());

        // Run constraint analysis.
        let constraint_analysis = run_constraint_analysis(&cfg);
        for (source, sinks) in sources.iter().zip(sinks) {
            assert_eq!(constraint_analysis.single_step_constraint(source).len(), *sinks)
        }
    }
}
