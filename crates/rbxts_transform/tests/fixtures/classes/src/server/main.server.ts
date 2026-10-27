import { Point, useClasses } from "shared/basics";
import { useDecorators } from "shared/decorators";
import DefaultClass from "shared/defaultClass";
import { useExpressions } from "shared/expressions";
import NamedDefaultClass, { useInheritance, useMore } from "shared/inheritance";
import { makeLocal, useMembers } from "shared/members";

export class NamedDefault {
	public static value = Point.dimensions;
}

print(Point, DefaultClass, NamedDefault);
useClasses();
useDecorators();
useExpressions();
useInheritance();
useMore();
useMembers();
print(makeLocal(), NamedDefaultClass);
