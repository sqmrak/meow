void f() {
%hook A
- (void)x { %orig; }
%end
}
