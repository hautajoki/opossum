// computed keys: offered to property_key with the key of a single string literal type, else `None` (declined)
const ka = "a";
const km = "m";
export const c1 = keyed[ka];
export const c2 = keyed[keyName() as "b"];
keyed[ka] = 1;
keyed[keyName() as "b"] += 2;
export const c3 = keyed[km](1);
export const c4 = keyed[keyName() as "m"](2);
export const c5 = keyed[km]?.(3);
export const c6 = maybeKeyed?.[ka];
export const c7 = keyed[anyKey as "a" | "b"];
export const c8 = keyedList[0];
delete keyed[keyName() as "opt"];
export const c9 = probeLinks();

// a rewritten key drops an argument that only reads (an identifier, an enum member); one with effects is still
// evaluated, as a call statement when it is a call behind parentheses or a cast
enum SK { A = "a", M = "m" }
namespace KeyNS { export enum Inner { B = "b" } }
export const c10 = keyed[SK.A];
export const c11 = keyed[SK.M](4);
export const c12 = maybeKeyed?.[KeyNS.Inner.B];
keyed[SK.A] = 5;
export const c13 = keyed[(keyName() as "a")];
export const c14 = keyed[keyed.s as "a"];
export const c15 = keyed[anyKey as "b"];

// optional chains with a rewritten computed key: the key is evaluated after its receiver, and not at all when an
// optional receiver is nil
let kv = 0;
export const o1 = getMaybeKeyed()?.[keyName() as "m"]?.(val());
export const o2 = getMaybeKeyed()?.[keyName() as "f"]?.(val());
export const o3 = getMaybeKeyed()?.[keyName() as "a"];
export const o4 = getMaybeKeyed()?.[keyName() as "m"](val());
export const o5 = getKeyed()[keyName() as "m"]?.(val());
export const o6 = getMaybeKeyed()?.inner[keyName() as "m"]?.(val());
export const o7 = getMaybeKeyed()?.[keyName() as "h"]?.(val())?.[keyName() as "g"]?.(val());
export const o8 = getMaybeKeyed()?.[keyName() as "g"](val())?.[keyName() as "h"]?.(val())[keyName() as "a"];
export const o9 = getMaybeKeyed()?.[(kv = 1, "m")]?.(val());
export const o10 = maybeKeyed?.[keyName() as "f"]?.(val());
export const o11 = getMaybeKeyed()?.["m"]?.(val());
getMaybeKeyed()?.[keyName() as "m"]?.(val());
delete getMaybeKeyed()?.[keyName() as "opt"];

interface CompAttrs {
	count: number;
	label: string;
}

// assignments with a computed key: the hook compiles `key_node`
export class CompKeys {
	/** @probeattrs */
	attrs: CompAttrs = { count: 0, label: "" };

	run(k: "count" | "label") {
		const kc = "count";
		this.attrs[kc] = 2;
		this.attrs[k] = 1 as never;
	}
}
