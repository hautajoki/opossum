// An entity past U+10FFFF, in a text child and in an attribute string: stock roblox-ts crashes the whole build (a
// RangeError); rbxts-rs reports an error for each and does not write the file.
import React from "shared/react";

export const text = <textlabel>&#x110000;</textlabel>;
export const attribute = <textlabel Text="&#1114112;" />;
