%hook   A  
-   ( void )   foo : ( int ) a   bar : ( id ) b   {
	%orig ( a , b ) ;
}
-(NSString*)bar{return %orig;}
%end
