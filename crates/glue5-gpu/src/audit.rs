//! Opt-in workload telemetry for the GM45 D3D11 feasibility investigation.
//! Compiled only with the `gpu-workload-audit` feature.
use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Clone, Copy, Debug)]
pub enum Access {
    Read,
    Write,
    ReadWrite,
}

#[derive(Clone, Copy, Debug)]
pub struct ResourceUse {
    pub id: u32,
    pub binding: u32,
    pub access: Access,
}

#[derive(Default)]
struct ComputeSummary {
    label: String,
    dispatches: u64,
    first_frame: u64,
    last_frame: u64,
    min_groups: [u32; 3],
    max_groups: [u32; 3],
    sum_groups: [u64; 3],
    // (WGSL binding, resource ID) -> number of dispatches using this pair.
    reads: FxHashMap<(u32, u32), u64>,
    writes: FxHashMap<(u32, u32), u64>,
}

#[derive(Default)]
struct Flow {
    count: u64,
    first_frame: u64,
    last_frame: u64,
}

impl Flow {
    fn record(&mut self, frame: u64) {
        if self.count == 0 {
            self.first_frame = frame;
        }
        self.last_frame = frame;
        self.count += 1;
    }
}

#[derive(Default)]
pub struct Audit {
    seen_shaders: FxHashSet<(u64, u8, u64)>,
    seen_resources: FxHashSet<u32>,
    compute: FxHashMap<u64, ComputeSummary>,
    // Resource ID -> last compute shader hash that wrote it. Kept across
    // reporting windows so a later draw can still be linked to its producer.
    last_compute_writer: FxHashMap<u32, u64>,
    compute_edges: FxHashMap<(u64, u64, u32), Flow>,
    draw_edges: FxHashMap<(u64, u64, u64, u32), Flow>,
}

impl Audit {
    pub fn first_shader(&mut self, hash: u64, stage: u8, variant: u64) -> bool {
        self.seen_shaders.insert((hash, stage, variant))
    }

    pub fn first_resource(&mut self, id: u32) -> bool {
        self.seen_resources.insert(id)
    }

    pub fn dispatch(&mut self, frame: u64, hash: u64, label: &str, groups: [u32; 3], resources: &[ResourceUse]) {
        let entry = self.compute.entry(hash).or_default();
        if entry.dispatches == 0 {
            entry.label = label.to_owned();
            entry.first_frame = frame;
            entry.min_groups = groups;
        }
        entry.last_frame = frame;
        entry.dispatches += 1;
        for i in 0..3 {
            entry.min_groups[i] = entry.min_groups[i].min(groups[i]);
            entry.max_groups[i] = entry.max_groups[i].max(groups[i]);
            entry.sum_groups[i] += u64::from(groups[i]);
        }

        for resource in resources {
            let binding_resource = (resource.binding, resource.id);
            match resource.access {
                Access::Read => {
                    *entry.reads.entry(binding_resource).or_insert(0) += 1;
                    if let Some(&writer) = self.last_compute_writer.get(&resource.id) {
                        self.compute_edges.entry((writer, hash, resource.id)).or_default().record(frame);
                    }
                }
                Access::Write => {
                    *entry.writes.entry(binding_resource).or_insert(0) += 1;
                    self.last_compute_writer.insert(resource.id, hash);
                }
                Access::ReadWrite => {
                    *entry.reads.entry(binding_resource).or_insert(0) += 1;
                    *entry.writes.entry(binding_resource).or_insert(0) += 1;
                    if let Some(&writer) = self.last_compute_writer.get(&resource.id) {
                        self.compute_edges.entry((writer, hash, resource.id)).or_default().record(frame);
                    }
                    self.last_compute_writer.insert(resource.id, hash);
                }
            }
        }
    }

    pub fn draw(&mut self, frame: u64, vs_hash: u64, ps_hash: u64, resources: &[u32]) {
        for &resource in resources {
            if let Some(&writer) = self.last_compute_writer.get(&resource) {
                self.draw_edges
                    .entry((writer, vs_hash, ps_hash, resource))
                    .or_default()
                    .record(frame);
            }
        }
    }

    /// Emit one compact report per normal GPU stats interval. No per-dispatch
    /// logging, so the capture has bounded overhead and remains readable.
    pub fn report(&mut self) {
        let mut rows: Vec<_> = self.compute.iter().collect();
        rows.sort_by(|a, b| b.1.dispatches.cmp(&a.1.dispatches));
        for (&hash, summary) in rows {
            let bindings = |map: &FxHashMap<(u32, u32), u64>| {
                let mut values: Vec<_> = map.iter().map(|(&(binding, id), count)| (binding, id, *count)).collect();
                values.sort_unstable_by_key(|v| (v.0, v.1));
                values
                    .iter()
                    .map(|(binding, id, count)| format!("{binding}:{id}:{count}"))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let avg = |i: usize| summary.sum_groups[i] as f64 / summary.dispatches.max(1) as f64;
            log::info!(
                "GLUE5_GPU_AUDIT|COMPUTE|hash={hash:016x}|name={}|dispatches={}|frames={}-{}|groups_avg={:.2},{:.2},{:.2}|groups_min={},{},{}|groups_max={},{},{}|reads_binding:id:count={}|writes_binding:id:count={}",
                summary.label.replace('|', "/").replace('\n', " "),
                summary.dispatches,
                summary.first_frame,
                summary.last_frame,
                avg(0),
                avg(1),
                avg(2),
                summary.min_groups[0],
                summary.min_groups[1],
                summary.min_groups[2],
                summary.max_groups[0],
                summary.max_groups[1],
                summary.max_groups[2],
                bindings(&summary.reads),
                bindings(&summary.writes),
            );
        }

        let mut compute_edges: Vec<_> = self.compute_edges.drain().collect();
        compute_edges.sort_unstable_by_key(|((writer, consumer, resource), _)| (*writer, *consumer, *resource));
        for ((writer, consumer, resource), flow) in compute_edges {
            log::info!(
                "GLUE5_GPU_AUDIT|FLOW_COMPUTE|writer={writer:016x}|reader={consumer:016x}|resource={resource}|count={}|frames={}-{}",
                flow.count,
                flow.first_frame,
                flow.last_frame,
            );
        }

        let mut draw_edges: Vec<_> = self.draw_edges.drain().collect();
        draw_edges.sort_unstable_by_key(|((writer, vs, ps, resource), _)| (*writer, *vs, *ps, *resource));
        for ((writer, vs, ps, resource), flow) in draw_edges {
            log::info!(
                "GLUE5_GPU_AUDIT|FLOW_DRAW|writer={writer:016x}|vs={vs:016x}|ps={ps:016x}|resource={resource}|draws={}|frames={}-{}",
                flow.count,
                flow.first_frame,
                flow.last_frame,
            );
        }
        self.compute.clear();
    }
}
