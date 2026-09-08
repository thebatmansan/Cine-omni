//! Backend-agnostic frame graph.
//!
//! A frame is described as passes reading and writing named resources. The
//! graph computes an execution order from data dependencies, culls passes that
//! do not contribute to the requested outputs, and assigns transient resources
//! to a minimal set of physical slots (aliasing by lifetime) so a 4K comp with
//! forty nodes does not allocate forty 4K buffers.

use crate::kernels::KernelId;
use crate::{GpuError, Result};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// RGBA f32 image, width × height.
    Image,
    /// Depth/ID pass.
    Depth,
    /// Arbitrary storage buffer (histograms, deep samples).
    Buffer,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResourceDesc {
    pub name: String,
    pub kind: ResourceKind,
    pub width: u32,
    pub height: u32,
    /// Bytes for `Buffer` resources.
    pub bytes: u64,
    /// Imported resources (inputs / final outputs) are never aliased.
    pub imported: bool,
}

impl ResourceDesc {
    pub fn image(name: &str, width: u32, height: u32) -> Self {
        ResourceDesc {
            name: name.into(),
            kind: ResourceKind::Image,
            width,
            height,
            bytes: 0,
            imported: false,
        }
    }
    pub fn imported(mut self) -> Self {
        self.imported = true;
        self
    }
    pub fn byte_size(&self) -> u64 {
        match self.kind {
            ResourceKind::Image => self.width as u64 * self.height as u64 * 16,
            ResourceKind::Depth => self.width as u64 * self.height as u64 * 16,
            ResourceKind::Buffer => self.bytes,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Pass {
    pub name: String,
    pub kernel: KernelId,
    pub reads: Vec<ResourceId>,
    pub writes: Vec<ResourceId>,
    /// Raw uniform bytes for the kernel.
    pub params: Vec<u8>,
    pub dispatch: [u32; 3],
}

#[derive(Clone, Debug, Default)]
pub struct FrameGraph {
    pub resources: Vec<ResourceDesc>,
    pub passes: Vec<Pass>,
}

/// The compiled execution plan.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Pass indices in execution order (culled passes omitted).
    pub order: Vec<usize>,
    /// Physical slot assigned to each transient resource.
    pub slots: HashMap<ResourceId, usize>,
    /// Size in bytes of each physical slot.
    pub slot_sizes: Vec<u64>,
    /// Bytes that would have been allocated without aliasing.
    pub naive_bytes: u64,
}

impl Plan {
    pub fn aliased_bytes(&self) -> u64 {
        self.slot_sizes.iter().sum()
    }
}

impl FrameGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_resource(&mut self, desc: ResourceDesc) -> ResourceId {
        self.resources.push(desc);
        ResourceId(self.resources.len() as u32 - 1)
    }

    pub fn add_pass(&mut self, pass: Pass) -> usize {
        self.passes.push(pass);
        self.passes.len() - 1
    }

    fn desc(&self, id: ResourceId) -> Result<&ResourceDesc> {
        self.resources
            .get(id.0 as usize)
            .ok_or_else(|| GpuError::Graph(format!("unknown resource {id:?}")))
    }

    /// Compile into an execution plan producing `outputs`.
    pub fn compile(&self, outputs: &[ResourceId]) -> Result<Plan> {
        for o in outputs {
            self.desc(*o)?;
        }
        // Producer of each resource (last writer wins; multiple writers are ordered by index).
        let mut producers: HashMap<ResourceId, Vec<usize>> = HashMap::new();
        for (i, p) in self.passes.iter().enumerate() {
            for w in &p.writes {
                self.desc(*w)?;
                producers.entry(*w).or_default().push(i);
            }
            for r in &p.reads {
                self.desc(*r)?;
            }
        }

        // Dependencies: pass depends on every producer of its reads that precedes it in
        // declaration order (or all producers if none precede — then it's a graph error
        // when a pass reads a resource written only later).
        let mut deps: Vec<Vec<usize>> = vec![Vec::new(); self.passes.len()];
        for (i, p) in self.passes.iter().enumerate() {
            for r in &p.reads {
                if let Some(ps) = producers.get(r) {
                    let before: Vec<usize> = ps.iter().copied().filter(|&j| j < i).collect();
                    if let Some(last) = before.last() {
                        deps[i].push(*last);
                    } else if !self.desc(*r)?.imported {
                        return Err(GpuError::Graph(format!(
                            "pass `{}` reads `{}` before it is written",
                            p.name,
                            self.desc(*r)?.name
                        )));
                    }
                }
            }
            // Write-after-write ordering.
            for w in &p.writes {
                if let Some(ps) = producers.get(w) {
                    for &j in ps.iter().filter(|&&j| j < i) {
                        deps[i].push(j);
                    }
                }
            }
        }

        // Culling: walk back from output producers.
        let mut needed: HashSet<usize> = HashSet::new();
        let mut stack: Vec<usize> = outputs
            .iter()
            .filter_map(|o| producers.get(o).and_then(|v| v.last().copied()))
            .collect();
        while let Some(i) = stack.pop() {
            if needed.insert(i) {
                stack.extend(deps[i].iter().copied());
            }
        }

        // Topological order among needed passes (Kahn, stable by index).
        let mut indeg: HashMap<usize, usize> = needed
            .iter()
            .map(|&i| (i, deps[i].iter().filter(|d| needed.contains(d)).count()))
            .collect();
        let mut order = Vec::new();
        let mut ready: Vec<usize> = indeg
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&i, _)| i)
            .collect();
        ready.sort_unstable();
        while !ready.is_empty() {
            let i = ready.remove(0);
            order.push(i);
            for (j, ds) in deps.iter().enumerate() {
                if needed.contains(&j) && ds.contains(&i) {
                    let e = indeg.get_mut(&j).unwrap();
                    *e -= 1;
                    if *e == 0 {
                        ready.push(j);
                        ready.sort_unstable();
                    }
                }
            }
        }
        if order.len() != needed.len() {
            return Err(GpuError::Graph("cycle detected in frame graph".into()));
        }

        // Lifetimes: first/last step in `order` each resource is used.
        let mut first: HashMap<ResourceId, usize> = HashMap::new();
        let mut last: HashMap<ResourceId, usize> = HashMap::new();
        for (step, &pi) in order.iter().enumerate() {
            for r in self.passes[pi]
                .reads
                .iter()
                .chain(self.passes[pi].writes.iter())
            {
                first.entry(*r).or_insert(step);
                last.insert(*r, step);
            }
        }
        // Greedy interval aliasing on transient resources (largest first).
        let mut transients: Vec<ResourceId> = first
            .keys()
            .copied()
            .filter(|r| !self.resources[r.0 as usize].imported)
            .collect();
        transients.sort_by(|a, b| {
            let sa = self.resources[a.0 as usize].byte_size();
            let sb = self.resources[b.0 as usize].byte_size();
            sb.cmp(&sa).then(a.cmp(b))
        });
        let mut slot_sizes: Vec<u64> = Vec::new();
        let mut slot_busy_until: Vec<Vec<(usize, usize)>> = Vec::new();
        let mut slots = HashMap::new();
        let mut naive_bytes = 0;
        for r in transients {
            let size = self.resources[r.0 as usize].byte_size();
            naive_bytes += size;
            let (f, l) = (first[&r], last[&r]);
            let mut chosen = None;
            for (si, intervals) in slot_busy_until.iter().enumerate() {
                let overlaps = intervals.iter().any(|&(a, b)| f <= b && a <= l);
                if !overlaps && slot_sizes[si] >= size {
                    chosen = Some(si);
                    break;
                }
            }
            let si = match chosen {
                Some(si) => si,
                None => {
                    slot_sizes.push(size);
                    slot_busy_until.push(Vec::new());
                    slot_sizes.len() - 1
                }
            };
            slot_busy_until[si].push((f, l));
            slots.insert(r, si);
        }
        Ok(Plan {
            order,
            slots,
            slot_sizes,
            naive_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(name: &str, kernel: KernelId, reads: &[ResourceId], writes: &[ResourceId]) -> Pass {
        Pass {
            name: name.into(),
            kernel,
            reads: reads.to_vec(),
            writes: writes.to_vec(),
            params: vec![],
            dispatch: [1, 1, 1],
        }
    }

    #[test]
    fn orders_culls_and_aliases() {
        let mut g = FrameGraph::new();
        let plate = g.add_resource(ResourceDesc::image("plate", 1920, 1080).imported());
        let key = g.add_resource(ResourceDesc::image("key", 1920, 1080));
        let blur1 = g.add_resource(ResourceDesc::image("blur_h", 1920, 1080));
        let blur2 = g.add_resource(ResourceDesc::image("blur_v", 1920, 1080));
        let unused = g.add_resource(ResourceDesc::image("unused", 1920, 1080));
        let out = g.add_resource(ResourceDesc::image("out", 1920, 1080).imported());
        // Declared out of order on purpose.
        g.add_pass(pass("merge", KernelId::Merge, &[plate, blur2], &[out]));
        g.add_pass(pass("key", KernelId::Keyer, &[plate], &[key]));
        g.add_pass(pass("blur_h", KernelId::Blur, &[key], &[blur1]));
        g.add_pass(pass("blur_v", KernelId::Blur, &[blur1], &[blur2]));
        g.add_pass(pass("dead", KernelId::Cdl, &[plate], &[unused]));
        // "merge" reads blur2 which is produced later in declaration order → error by design.
        assert!(g.compile(&[out]).is_err());

        let mut g = FrameGraph::new();
        let plate = g.add_resource(ResourceDesc::image("plate", 1920, 1080).imported());
        let key = g.add_resource(ResourceDesc::image("key", 1920, 1080));
        let blur1 = g.add_resource(ResourceDesc::image("blur_h", 1920, 1080));
        let blur2 = g.add_resource(ResourceDesc::image("blur_v", 1920, 1080));
        let unused = g.add_resource(ResourceDesc::image("unused", 1920, 1080));
        let out = g.add_resource(ResourceDesc::image("out", 1920, 1080).imported());
        g.add_pass(pass("key", KernelId::Keyer, &[plate], &[key]));
        g.add_pass(pass("blur_h", KernelId::Blur, &[key], &[blur1]));
        g.add_pass(pass("dead", KernelId::Cdl, &[plate], &[unused]));
        g.add_pass(pass("blur_v", KernelId::Blur, &[blur1], &[blur2]));
        g.add_pass(pass("merge", KernelId::Merge, &[plate, blur2], &[out]));
        let plan = g.compile(&[out]).unwrap();
        assert_eq!(
            plan.order,
            vec![0, 1, 3, 4],
            "dead pass culled, order preserved"
        );
        // key, blur_h, blur_v are transients with non-overlapping lifetimes except adjacent ones:
        // key[0..1], blur_h[1..3], blur_v[3..4] → key and blur_v can share a slot.
        assert_eq!(plan.slot_sizes.len(), 2);
        assert_eq!(plan.slots[&key], plan.slots[&blur2]);
        assert!(plan.aliased_bytes() < plan.naive_bytes);
    }
}
