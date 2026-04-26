fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::compile_protos("proto/glasschain/v1/glasschain.proto")?;
    Ok(())
}
