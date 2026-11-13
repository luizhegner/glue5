use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        return Err("usage: naga-hlsl-probe <input.wgsl> <vs|ps> <output.hlsl>".into());
    }

    let source = fs::read_to_string(&args[1])?;
    let stage = match args[2].as_str() {
        "vs" => naga::ShaderStage::Vertex,
        "ps" => naga::ShaderStage::Fragment,
        _ => return Err("stage must be vs or ps".into()),
    };

    let module = naga::front::wgsl::parse_str(&source)
        .map_err(|e| format!("WGSL parse: {}", e.emit_to_string(&source)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)?;

    let mut options = naga::back::hlsl::Options {
        shader_model: naga::back::hlsl::ShaderModel::V5_0,
        ..Default::default()
    };

    // GTA's WGSL binding convention encodes D3D11 resource slots:
    // bN => binding N, sN => binding 16+N, tN => binding 32+N.
    // Each shader stage gets its own D3D11 register namespace.
    for (_, global) in module.global_variables.iter() {
        let Some(binding) = global.binding else {
            continue;
        };
        let inner = &module.types[global.ty].inner;
        let register = match inner {
            naga::TypeInner::Sampler { .. } => binding.binding.checked_sub(16)
                .ok_or("sampler binding is below GTA's 16+slot range")?,
            naga::TypeInner::Image { .. } => binding.binding.checked_sub(32)
                .ok_or("texture binding is below GTA's 32+slot range")?,
            _ => binding.binding,
        };

        options.binding_map.insert(binding, naga::back::hlsl::BindTarget {
            space: 0,
            register,
            binding_array_size: None,
            dynamic_storage_buffer_offsets_index: None,
            restrict_indexing: false,
        });
    }

    let pipeline = naga::back::hlsl::PipelineOptions {
        entry_point: Some((stage, "main".into())),
    };
    let mut generated = String::new();
    let mut writer = naga::back::hlsl::Writer::new(&mut generated, &options, &pipeline);
    writer.write(&module, &info, None)?;

    // Naga routes WebGPU scalar samplers through a sampler heap and an
    // index buffer. GTA's statically bound scalar samplers map directly to sN.
    let mut output = String::new();
    for line in generated.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("SamplerState nagaSamplerHeap[")
            || trimmed.starts_with("SamplerComparisonState nagaComparisonSamplerHeap[")
            || trimmed.starts_with("StructuredBuffer<uint> nagaGroup")
        {
            continue;
        }

        if (trimmed.starts_with("static const SamplerState ")
            || trimmed.starts_with("static const SamplerComparisonState "))
            && (trimmed.contains("nagaSamplerHeap[")
                || trimmed.contains("nagaComparisonSamplerHeap["))
        {
            let ty = if trimmed.starts_with("static const SamplerComparisonState ") {
                "SamplerComparisonState"
            } else {
                "SamplerState"
            };
            let name = trimmed.split_whitespace().nth(3)
                .ok_or("could not read generated sampler variable name")?;
            let marker = "Array[";
            let marker_pos = trimmed.rfind(marker)
                .ok_or("could not read generated sampler binding index")?;
            let slot_text = trimmed[marker_pos + marker.len()..]
                .split(']')
                .next()
                .ok_or("could not parse generated sampler binding index")?;
            let slot: u32 = slot_text.parse()?;
            output.push_str(&format!("{ty} {name} : register(s{slot});\n"));
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    fs::write(&args[3], &output)?;
    if output.contains("nagaSamplerHeap")
        || output.contains("nagaComparisonSamplerHeap")
        || output.contains("nagaGroup")
        || output.contains("space")
    {
        return Err("SM4 output still contains a WebGPU sampler heap, group index buffer, or register space".into());
    }
    Ok(())
}
