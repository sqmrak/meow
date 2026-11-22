%hook A
- (void)x { %orig; }
%end
%group G
%hook A
- (void)y { %orig; }
%end
%end
%ctor { %init; %init(G); }
