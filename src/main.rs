pub mod feature_extractor_pipeline;
pub mod util;
pub mod wgsl;
pub mod decision_tree;

use decision_tree::RandomForest;
use feature_extractor_pipeline::{kernel::gaussian_blur::GaussianBlur, pipeline::FeatureExtractorPipeline};
use pollster::FutureExt;
use rand::RngCore;
use util::{copy_bytes, timeit, ImageBufferExt, WorkgroupSize};
use wgpu::Extent3d;

use clap::Parser;


fn make_pipeline<const KSIDE: usize>(
    forest: &RandomForest,
    kernels: Vec<GaussianBlur<KSIDE>>,
    img_extent: Extent3d,
) -> FeatureExtractorPipeline<KSIDE> {
    // We first initialize an wgpu `Instance`, which contains any "global" state wgpu needs.
    //
    // This is what loads the vulkan/dx12/metal/opengl libraries.
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor{
        // flags: wgpu::InstanceFlags::debugging(),
        ..Default::default()
    });

    // We then create an `Adapter` which represents a physical gpu in the system. It allows
    // us to query information about it and create a `Device` from it.
    //
    // This function is asynchronous in WebGPU, so request_adapter returns a future. On native/webgl
    // the future resolves immediately, so we can block on it without harm.
    let adapter = instance.request_adapter(
            &wgpu::RequestAdapterOptions{
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            }
        )
        .block_on()
        .expect("Failed to create adapter");

    // Print out some basic information about the adapter.
    println!("Running on Adapter: {:#?}", adapter.get_info());

    // Check to see if the adapter supports compute shaders. While WebGPU guarantees support for
    // compute shaders, wgpu supports a wider range of devices through the use of "downlevel" devices.
    let downlevel_capabilities = adapter.get_downlevel_capabilities();
    if !downlevel_capabilities
        .flags
        .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
    {
        panic!("Adapter does not support compute shaders");
    }

    // We then create a `Device` and a `Queue` from the `Adapter`.
    //
    // The `Device` is used to create and manage GPU resources.
    // The `Queue` is a queue used to submit work for the GPU to process.
    let (device, queue) = adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        },
    )
    .block_on()
    .expect("Failed to create device");

    FeatureExtractorPipeline::new(
        device,
        queue,
        WorkgroupSize{
            x: 16,
            y: 16,
            z: 1,
        },
        kernels,
        forest,
        img_extent,
    )
}

fn main() {
    const KERNEL_SIDE: usize = 73;

    // wgpu uses `log` for all of our logging, so we initialize a logger with the `env_logger` crate.
    //
    // To change the log level, set the `RUST_LOG` environment variable. See the `env_logger`
    // documentation for more information.
    env_logger::init();


    let forest: RandomForest = RandomForest::from_dir("./bench/out/benchmark_trees").unwrap();
    eprintln!("HIgest feat idx is {}", forest.highest_feature_idx());


    // let img = image::io::Reader::open("./big.png").unwrap().decode().unwrap();
    let img = image::io::Reader::open("./c_cells_1_big_3ch.png").unwrap().decode().unwrap();
    let image = img.to_rgba8();
    
    // let img = image::io::Reader::open("./big2.png").unwrap().decode().unwrap();
    // let image = img.to_rgba8();

    // let img = image::io::Reader::open("./big3.png").unwrap().decode().unwrap();
    // let image = img.to_rgba8();

    // let image = {
    //     const WIDTH: usize = 2048;
    //     const HEIGHT: usize = 2048;
    //     const NUM_CHANNELS: usize = 4; //FIXME

    //     let mut rng = rand::rng();
    //     let mut test_source: Vec<u8> = vec![0; WIDTH * HEIGHT * NUM_CHANNELS * size_of::<f32>()];
    //     rng.fill_bytes(&mut test_source);
    //     // let mut test_sink: Vec<u8> = vec![0; WIDTH * HEIGHT * NUM_CHANNELS];

    //     let _aaa: Vec<[f32; 4]> = copy_bytes(&test_source, "from cpu to cpu");

    //     let image: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> = {
    //         let mut bytes: Vec<u8> = vec![0; WIDTH * HEIGHT * NUM_CHANNELS];
    //         rng.fill_bytes(&mut bytes);
    //         image::ImageBuffer::from_raw(WIDTH as u32, HEIGHT as u32, bytes).unwrap()
    //     };
    //     image
    // };

    let dims = image.dimensions();
    println!("Image has these dimensions: {:?} ", dims);

    let kernels = vec![
        // 0.3, 0.7, 0.9, 1.0, 1.6, 3.5, 4.0, 5.0, 7.0, 10.0
        GaussianBlur::<KERNEL_SIDE>{ sigma: 0.3 },
        GaussianBlur::<KERNEL_SIDE>{ sigma: 0.7 },
        GaussianBlur::<KERNEL_SIDE>{ sigma: 0.9, },
        GaussianBlur::<KERNEL_SIDE>{ sigma: 1.0, },
        GaussianBlur::<KERNEL_SIDE>{ sigma: 1.6 },
        GaussianBlur::<KERNEL_SIDE>{ sigma: 3.5 },
        GaussianBlur::<KERNEL_SIDE>{ sigma: 4.0 },
        GaussianBlur::<KERNEL_SIDE>{ sigma: 5.0 },
        GaussianBlur::<KERNEL_SIDE>{ sigma: 7.0 },
        GaussianBlur::<KERNEL_SIDE>{ sigma: 10.0 },
    ];

    let num_kernels = kernels.len();

    let pipeline = make_pipeline(&forest, kernels.clone(), image.extent());

    let width = image.width();
    let height = image.height();
    let predictions: Vec<[f32; 4]> = timeit(
        &format!("Convo a {width}x{height}x3c img with {num_kernels} kernel(s) of {KERNEL_SIDE}^2"),
        || pipeline.process(&image).unwrap()
    );

    let num_pixels = (width * height) as usize;
    let img_slice = &predictions[0..num_pixels];
    let img_slice_f32: &[f32] = bytemuck::cast_slice(img_slice);
    assert!(img_slice_f32.len() == num_pixels * 4);

    let rgba_u8: Vec<u8> = img_slice_f32.iter()
        .map(|channel| (*channel * 255.0) as u8)
        .collect::<Vec<_>>();
    let parsed = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(width, height, rgba_u8)
        .expect("Could not parse rgba u8 image!!!");
    parsed.save(format!("./bench/out/segmentation_gpu.png")).unwrap();
}
