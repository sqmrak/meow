#import <Foundation/Foundation.h>
#import "@PREFIX@RootListController.h"

@implementation @PREFIX@RootListController

- (NSArray *)specifiers {
	if (!_specifiers) {
		_specifiers = [[self loadSpecifiersFromPlistName:@"Root" target:self] retain];
	}
	return _specifiers;
}

@end
