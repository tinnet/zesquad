fn main() {
    shadow_rs::ShadowBuilder::builder()
        .build()
        .expect("shadow-rs could not read the build environment");
}
