// Node imports are lazy; shared browser bundles never need these modules.
const $processError = (kind, error) => $sum("Error", $record([["kind", $sum(kind, undefined)], ["message", $webMessage(error)]]));
const $process = {
  args: () => process.argv.slice(2),
  env: name => {
    if (!name || name.includes("=") || name.includes("\0")) return $processError("InvalidName", "Invalid environment variable name");
    try {
      const value = Object.hasOwn(process.env, name) ? process.env[name] : undefined;
      return $sum("Some", value === undefined ? $sum("None", undefined) : $sum("Some", value));
    } catch (error) { return $processError("Other", error); }
  },
  cwd: () => {
    try { return $sum("Some", process.cwd()); }
    catch (error) { return $processError("Unavailable", error); }
  }
};
let $pathModule;
const $loadPath = () => $pathModule || ($pathModule = import("node:path"));
const $pathFlavor = flavor => {
  const api = {};
  for (const name of ["normalize", "basename", "dirname", "extname", "isAbsolute", "parse", "join"]) {
    api[name] = async input => {
      const module = await $loadPath();
      const path = flavor === "native" ? module : module[flavor];
      return name === "join" ? path.join(...input) : path[name](input);
    };
  }
  return api;
};
const $pathCall = async operation => {
  try { return $sum("Some", await operation(await $loadPath())); }
  catch (error) { return $sum("Error", $record([["message", $webMessage(error)]])); }
};
const $nodePath = {
  native: $pathFlavor("native"), posix: $pathFlavor("posix"), win32: $pathFlavor("win32"),
  resolve: parts => $pathCall(path => path.resolve(...parts)),
  relative: request => $pathCall(path => path.relative(request.from, request.to))
};
