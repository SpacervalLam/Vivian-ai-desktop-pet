fn main() {
    let base = "vendor/speexdsp";
    let mut build = cc::Build::new();
    build
        .include(format!("{base}/include"))
        .include(format!("{base}/libspeexdsp"))
        .define("FLOATING_POINT", None)
        .define("USE_KISS_FFT", None)
        .define("EXPORT", "")
        .warnings(false)
        .opt_level(2);
    for file in [
        "mdf.c",
        "fftwrap.c",
        "kiss_fft.c",
        "kiss_fftr.c",
        "preprocess.c",
        "filterbank.c",
    ] {
        build.file(format!("{base}/libspeexdsp/{file}"));
    }
    build.compile("vivian_speexdsp");
    println!("cargo:rerun-if-changed={base}");
}
