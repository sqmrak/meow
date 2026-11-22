%hook A
- (void)x { %orig; }
%end
%ctor {
	%init(A = objc_getClass("B"), junk, Missing = 1, =);
}
