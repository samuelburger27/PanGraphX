//! Assignment of odgi node ids (and ranks) to nodes.

use crate::core::core_types::NodeId;
use crate::core::graph_dto::CoreGraphDTO;
use crate::formats::odgi::VACANT;
use log::warn;
use rayon::prelude::*;

/// Assignment of odgi node ranks (`odgi id - 1`) to nodes.
pub(super) struct Numbering {
    /// odgi id of every node, `None` for `1..=n` in node order.
    pub(super) ids: Option<Vec<u64>>,
    /// Node at every rank (`VACANT` for gaps), `None` for `1..=n` in node order.
    pub(super) by_rank: Option<Vec<NodeId>>,
    /// Number of node records, including gaps.
    pub(super) records: usize,
}

impl Numbering {
    /// Keeps numeric node names as odgi ids when they are distinct, positive,
    /// written canonically and dense enough that the gaps stay small.
    pub(super) fn new(graph: &CoreGraphDTO) -> Self {
        let n = graph.nodes().len();
        let sequential = Self {
            ids: None,
            by_rank: None,
            records: n,
        };
        let Some(map) = graph.node_name_map() else {
            return sequential;
        };
        let ids: Option<Vec<u64>> = (0..n)
            .into_par_iter()
            .map(|node| {
                let name = map.get(&node)?;
                let id = std::str::from_utf8(name).ok()?.parse::<u64>().ok()?;
                (id > 0 && itoa::Buffer::new().format(id).as_bytes() == name.as_slice())
                    .then_some(id)
            })
            .collect();
        let named = ids.and_then(|ids| {
            let max = usize::try_from(*ids.iter().max()?).ok()?;
            if max > n.saturating_mul(2).saturating_add(1024) {
                return None;
            }
            let mut by_rank = vec![VACANT; max];
            for (node, &id) in ids.iter().enumerate() {
                let slot = &mut by_rank[usize::try_from(id - 1).ok()?];
                if *slot != VACANT {
                    return None;
                }
                *slot = node;
            }
            Some(Self {
                ids: Some(ids),
                by_rank: Some(by_rank),
                records: max,
            })
        });
        named.unwrap_or_else(|| {
            if map.values().any(|name| !name.is_empty()) {
                warn!(
                    "ODGI needs positive integer node ids; node names are replaced by ids 1..={n}"
                );
            }
            sequential
        })
    }

    pub(super) fn id(&self, node: NodeId) -> u64 {
        self.ids.as_ref().map_or(node as u64 + 1, |ids| ids[node])
    }

    pub(super) fn node_at(&self, rank: usize) -> Option<NodeId> {
        self.by_rank.as_ref().map_or(Some(rank), |by_rank| {
            Some(by_rank[rank]).filter(|&node| node != VACANT)
        })
    }
}
