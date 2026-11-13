// Exercise the exact vendored parser from the application's test target; the
// component fork is deliberately excluded from the Atlas workspace.
use gpui_component::Colorize;

mod color_input {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/gpui-component-fork/crates/ui/src/color_picker/color_input.rs"
    ));
}
