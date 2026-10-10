TODOs:
- Sequence search / k-mer query — pangraphx search -i g.gfa --query ACGT.... Reuses Kmer encoding directly. Find which nodes/paths contain a query; output matches with coordinates. Very cheap to build, high demo value.
 - Subgraph extraction — extract command: pull a named path, a node's k-hop neighborhood, or a component into a new graph. Easy given CoreGraph adjacency list + union-find in stats.rs.
 - Graph visualization (DOT export) — add GraphFormat::Dot as a serializer-only codec (fits your trait pattern perfectly). Produces thesis figures with zero external runtime deps.