// A TypeScript transformer that changes nothing: stock roblox-ts then prints each file again.
module.exports = () => () => file => file;
