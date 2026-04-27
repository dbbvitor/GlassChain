fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .compile_protos(
            &["proto/glasschain/v1/glasschain.proto"],
            &["proto"],
        )?;
    Ok(())
}
