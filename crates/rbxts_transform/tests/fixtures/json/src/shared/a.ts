import data from "./data.json";
import * as list from "./cfg/list.json";
import { speed, nested } from "./data.json";
import settings from "../../config/settings.json";

// exported functions first: TypeScript 5 lists them first in the export table, TypeScript 7 in source order
export async function load() {
	const lazy = await import("./data.json");
	return lazy.default.name;
}

export const first = list[0] + data.speed;
export const named = speed + (nested.on ? 1 : 0);
export const gravity = settings.gravity;
export const tagCount = data.tags.size();
