fn main() -> anyhow::Result<()> {
    build_data::set_GIT_BRANCH();
    build_data::set_GIT_COMMIT();
    build_data::set_SOURCE_TIMESTAMP();
    build_data::set_RUSTC_VERSION();
    build_data::no_debug_rebuilds();
    embuild::espidf::sysenv::output();
    Ok(())
}
