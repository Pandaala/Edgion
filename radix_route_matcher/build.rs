fn main() {
    cc::Build::new().file("csrc/rax.c").include("csrc").compile("rax");
}
