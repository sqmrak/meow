// behavior where logos-rust differs on purpose from the perl logos, see the readme

use crate::logos::{process, Config};

fn run(src: &str) -> Result<String, String> {
    process(src, "t.x", &Config::default())
        .map(|o| o.text)
        .map_err(|f| f.fatal.unwrap_or_default())
}

fn run_gen(src: &str, generator: &str) -> Result<String, String> {
    let config = Config { generator: generator.to_string(), warnings: "default".to_string() };
    process(src, "t.x", &config).map(|o| o.text).map_err(|f| f.fatal.unwrap_or_default())
}

#[test]
fn orig_call_that_continues_on_the_next_lines() {
    let src = "%hook A\n- (void)x:(id)a {\n\t%orig(^{\n\t\tNSLog(@\"a\");\n\t}, 1);\n}\n%end\n";
    let out = run(src).unwrap();
    assert!(
        out.contains("\t_logos_orig$_ungrouped$A$x$(self, _cmd, ^{ NSLog(@\"a\"); }, 1)\n\n;\n"),
        "{}",
        out
    );
    // the consumed lines stay in place so the line directives keep matching the source
    let body = out.split("id a) {\n").nth(1).unwrap().split("\n}\n").next().unwrap();
    assert_eq!(body.lines().count(), 3, "{}", body);
}

#[test]
fn text_after_a_multi_line_orig_call_is_kept() {
    let src = "%hook A\n- (int)x {\n\treturn %orig(^{\n\t\tfoo();\n\t}) + 1;\n}\n%end\n";
    let out = run(src).unwrap();
    assert!(out.contains("(self, _cmd, ^{ foo(); })\n\n + 1;\n"), "{}", out);
}

#[test]
fn unclosed_orig_call_is_an_error_at_its_line() {
    let src = "%hook A\n- (void)x {\n\t%orig(^{\n}\n%end\n";
    assert_eq!(run(src).unwrap_err(), "t.x:3: error: missing closing parenthesis");
}

#[test]
fn log_outside_a_method_is_an_error() {
    let src = "%hook A\n%log;\n%end\n";
    assert_eq!(run(src).unwrap_err(), "t.x:2: error: %log does not make sense outside a method");
}

#[test]
fn hookf_needs_a_type_and_a_name() {
    let msg = "error: %hookf needs (return type, function name, arguments...)";
    assert!(run("%hookf\n").unwrap_err().ends_with(msg));
    assert!(run("%hookf(int)\n").unwrap_err().ends_with(msg));
}

#[test]
fn raw_integer_log_arguments_do_not_leak_a_flag_into_the_format() {
    let src = "%hook A\n- (void)x {\n\t%log((MyType)3);\n}\n%end\n";
    let out = run(src).unwrap();
    assert!(out.contains("NSLog(@\"-[<A: %p> x]: 0x%llx\", self, (uint64_t)((MyType)3));"), "{}", out);
}

#[test]
fn hash_ordered_lists_keep_first_use_order() {
    let src = "%subclass S : NSObject <NSCopying, NSCoding>\n%end\n%hook Z\n- (void)a { %orig; }\n%end\n%ctor { %init; }\n";
    let out = run(src).unwrap();
    assert!(out.contains("@class NSObject; @class S; @class Z; \n"), "{}", out);
    let copying = out.find("objc_getProtocol(\"NSCopying\")").unwrap();
    let coding = out.find("objc_getProtocol(\"NSCoding\")").unwrap();
    assert!(copying < coding);
}

#[test]
fn class_directive_looks_up_the_class_it_names() {
    let out = run("%class NSString\n%class +NSArray\n%hook A\n- (void)x { }\n%end\n").unwrap();
    assert!(out.contains("objc_getClass(\"NSString\")"), "{}", out);
    assert!(out.contains("objc_getMetaClass(\"NSArray\")"), "{}", out);
}

#[test]
fn unknown_generator_is_reported() {
    let src = "%hook A\n- (void)x { }\n%end\n";
    assert_eq!(run_gen(src, "bogus").unwrap_err(), "t.x: error: I can't find the bogus Generator!");
    assert_eq!(
        run_gen(src, "mine@/some/path").unwrap_err(),
        "t.x: error: generators loaded from a path are not supported: mine@/some/path"
    );
}

#[test]
fn input_without_directives_only_gets_a_line_directive() {
    assert_eq!(run("int x;\n").unwrap(), "#line 1 \"t.x\"\nint x;\n");
    assert_eq!(run("").unwrap(), "#line 1 \"t.x\"\n");
}

#[test]
fn a_spaced_address_of_orig_is_replaced_whole() {
    let src = "%hookf(int, f, int a) {\n\tint (*p)(int) = & %orig;\n\treturn p(a);\n}\n%ctor { %init; }\n";
    let out = run(src).unwrap();
    assert!(out.contains("int (*p)(int) = _logos_orig$_ungrouped$f;"), "{}", out);
}
