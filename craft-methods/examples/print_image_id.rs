//! Print the compiled craft-guest's `image_id` in the 64-char hex form
//! that `synchronizer/relayer` expect for the `GUEST_IMAGE_ID` env var.

fn main() {
    println!("{}", craft_methods::image_id_hex());
}
