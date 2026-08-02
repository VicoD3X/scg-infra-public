use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSpec {
    pub id: String,
    pub dependencies: Vec<String>,
}

impl ServiceSpec {
    pub fn new<I, S>(id: impl Into<String>, dependencies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            id: id.into(),
            dependencies: dependencies.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServiceGraph {
    ordered: Vec<ServiceSpec>,
}

impl ServiceGraph {
    pub fn new(services: Vec<ServiceSpec>) -> CoreResult<Self> {
        if services.is_empty() {
            return Err(CoreError::InvalidServiceGraph(
                "at least one service is required".to_owned(),
            ));
        }

        let mut by_id = BTreeMap::new();
        for service in services {
            if service.id.trim().is_empty() {
                return Err(CoreError::InvalidServiceGraph(
                    "service identifiers cannot be empty".to_owned(),
                ));
            }
            if by_id.insert(service.id.clone(), service).is_some() {
                return Err(CoreError::InvalidServiceGraph(
                    "service identifiers must be unique".to_owned(),
                ));
            }
        }

        for service in by_id.values() {
            for dependency in &service.dependencies {
                if !by_id.contains_key(dependency) {
                    return Err(CoreError::InvalidServiceGraph(format!(
                        "service '{}' depends on missing service '{dependency}'",
                        service.id
                    )));
                }
            }
        }

        let mut indegree = BTreeMap::new();
        let mut dependents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for service in by_id.values() {
            indegree.insert(service.id.clone(), service.dependencies.len());
            for dependency in &service.dependencies {
                dependents
                    .entry(dependency.clone())
                    .or_default()
                    .insert(service.id.clone());
            }
        }

        let mut ready: VecDeque<String> = indegree
            .iter()
            .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
            .collect();
        let mut ordered = Vec::with_capacity(by_id.len());

        while let Some(id) = ready.pop_front() {
            ordered.push(by_id[&id].clone());
            if let Some(children) = dependents.get(&id) {
                for child in children {
                    let count = indegree.get_mut(child).expect("validated service graph");
                    *count -= 1;
                    if *count == 0 {
                        ready.push_back(child.clone());
                    }
                }
            }
        }

        if ordered.len() != by_id.len() {
            return Err(CoreError::InvalidServiceGraph(
                "service dependencies contain a cycle".to_owned(),
            ));
        }

        Ok(Self { ordered })
    }

    pub fn control_plane() -> Self {
        Self::new(vec![
            ServiceSpec::new("ingress", [] as [&str; 0]),
            ServiceSpec::new("policy", ["ingress"]),
            ServiceSpec::new("workers", ["policy"]),
            ServiceSpec::new("projection", ["workers"]),
            ServiceSpec::new("telemetry", ["ingress"]),
        ])
        .expect("built-in graph is valid")
    }

    pub fn start_order(&self) -> &[ServiceSpec] {
        &self.ordered
    }

    pub fn stop_order(&self) -> impl DoubleEndedIterator<Item = &ServiceSpec> {
        self.ordered.iter().rev()
    }
}

#[cfg(test)]
mod tests {
    use super::{ServiceGraph, ServiceSpec};

    #[test]
    fn sorts_services_in_dependency_order() {
        let graph = ServiceGraph::new(vec![
            ServiceSpec::new("api", ["core"]),
            ServiceSpec::new("core", [] as [&str; 0]),
            ServiceSpec::new("events", ["core"]),
        ])
        .unwrap();

        assert_eq!(graph.start_order()[0].id, "core");
        assert_eq!(graph.start_order().len(), 3);
    }

    #[test]
    fn rejects_cycles_and_missing_dependencies() {
        assert!(
            ServiceGraph::new(vec![
                ServiceSpec::new("a", ["b"]),
                ServiceSpec::new("b", ["a"]),
            ])
            .is_err()
        );
        assert!(ServiceGraph::new(vec![ServiceSpec::new("a", ["missing"])]).is_err());
    }
}
