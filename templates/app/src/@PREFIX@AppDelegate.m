#import "@PREFIX@AppDelegate.h"

// manual reference counting, the project does not set arc
@implementation @PREFIX@AppDelegate

- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)options {
	_window = [[UIWindow alloc] initWithFrame:[[UIScreen mainScreen] bounds]];
	_window.backgroundColor = [UIColor whiteColor];

	UILabel *label = [[UILabel alloc] initWithFrame:_window.bounds];
	label.text = @"@NAME@";
	label.textAlignment = UITextAlignmentCenter;
	label.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
	[_window addSubview:label];
	[label release];

	[_window makeKeyAndVisible];
	return YES;
}

- (void)dealloc {
	[_window release];
	[super dealloc];
}

@end
