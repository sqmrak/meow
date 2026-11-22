%hook A
%property (nonatomic, assign) char **names;
%property (nonatomic, assign) struct foo *thing;
%property (nonatomic, retain) NSDictionary<NSString *, id> *dict;
%property (nonatomic, copy) void (^block)(void);
%property(nonatomic,assign)int x;
%property (nonatomic, assign) unsigned long long big;
%property (nonatomic, strong) NSString * spaced ;
%end
