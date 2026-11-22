%group G
%hook A
- (void)x { %orig; }
%end
%end

%group G
%hook B
- (void)y { %orig; }
%end
%end

%ctor { %init(G); }
