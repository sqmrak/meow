%hook A
- (void)x { %orig; }
%end %hook B
- (void)y { %orig; }
%end
