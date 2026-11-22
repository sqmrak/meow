%group G
%hook A
- (void)x { %orig; }
%end
%end
%group H
%hook B
- (void)y { %orig; }
+ (void)z { %orig; }
%end
%end
%ctor {
	%init(_ungrouped);
	%init(G, A = objc_getClass("RA"));
	%init(H, +B = objc_getClass("RB"), B = objc_getClass("RC"));
}
