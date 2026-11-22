#[no_mangle]
pub extern "C" fn @PREFIX@_add(a: i32, b: i32) -> i32 {
	a.wrapping_add(b)
}
