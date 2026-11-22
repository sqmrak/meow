%hook A
-(void)foo:(int)a bar:(id)b{%orig;}
+(id)baz{return %orig;}
%end
