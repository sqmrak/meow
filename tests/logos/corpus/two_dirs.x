%group G %hook A
- (void)x { %orig; }
%end %end
%ctor { %init(G); }
