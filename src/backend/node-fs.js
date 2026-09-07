// Node's whole-file adapter. Import lazily so installing the platform handler
// does not access the filesystem or load Node modules until an operation runs.
let $fsModule;
const $nodeExit = code => new Promise(() => {
  let pending = 2;
  const drained = () => { if (--pending === 0) process.exit(Math.min(255, code)); };
  process.stdout.write("", drained);
  process.stderr.write("", drained);
});
const $fsErrorKinds = {
  ENOENT: "NotFound",
  EACCES: "PermissionDenied",
  EPERM: "PermissionDenied",
  EEXIST: "AlreadyExists",
  ENOTDIR: "NotDirectory",
  EISDIR: "IsDirectory",
  ENOTEMPTY: "DirectoryNotEmpty",
  EINVAL: "InvalidPath",
  ENAMETOOLONG: "InvalidPath",
  ERR_INVALID_ARG_VALUE: "InvalidPath",
  ERR_ENCODING_INVALID_ENCODED_DATA: "InvalidEncoding",
  ENOSYS: "Unsupported",
  ENOTSUP: "Unsupported",
  EOPNOTSUPP: "Unsupported"
};
const $fsFailure = (code, message, path) => Object.assign(new Error(message), { code, path });
const $fsDecode = bytes => new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
const $fsEncode = text => {
  const bytes = new TextEncoder().encode(text);
  if ($fsDecode(bytes) !== text) {
    throw $fsFailure("ERR_ENCODING_INVALID_ENCODED_DATA", "Text contains an unpaired surrogate");
  }
  return bytes;
};
const $fsPath = path => {
  // Do not silently replace unrepresentable characters in a filesystem path.
  if (path.includes("\0") || $fsDecode(new TextEncoder().encode(path)) !== path) {
    throw $fsFailure("EINVAL", "Path contains a NUL or an unpaired surrogate", path);
  }
  return path;
};
const $fsKind = info => $sum(info.isSymbolicLink() ? "Symlink"
  : info.isFile() ? "File" : info.isDirectory() ? "Directory" : "Other", undefined);
const $fsMetadata = info => $record([["kind", $fsKind(info)], ["size", info.size]]);
const $fsCall = async (path, operation) => {
  try {
    $fsPath(path);
    if (!$fsModule) $fsModule = import("node:fs/promises");
    const value = await operation(await $fsModule);
    return $sum("Some", value === undefined ? $record([]) : value);
  } catch (error) {
    const kind = Object.prototype.hasOwnProperty.call($fsErrorKinds, error.code)
      ? $fsErrorKinds[error.code] : "Other";
    return $sum("Error", $record([
      ["kind", $sum(kind, undefined)],
      ["path", typeof error.path === "string" ? error.path : path],
      ["message", String(error.message)]
    ]));
  }
};
const $fsRead = (path, decode) => $fsCall(path, async fs => {
  const file = await fs.open(path, "r");
  try {
    if ((await file.stat()).isDirectory()) {
      throw $fsFailure("EISDIR", "Cannot read a directory as a file", path);
    }
    return decode(await file.readFile());
  } finally {
    await file.close();
  }
});
const $fsByteList = bytes => {
  let result = $sum("None", undefined);
  for (let index = bytes.length - 1; index >= 0; index--) {
    result = $sum("Cons", $record([["0", bytes[index]], ["1", result]]));
  }
  return result;
};
const $fsByteBuffer = bytes => {
  let length = 0;
  for (let rest = bytes; rest[$tag] === "Cons"; rest = rest[$payload]["1"]) length++;
  const buffer = new Uint8Array(length);
  let index = 0;
  for (let rest = bytes; rest[$tag] === "Cons"; rest = rest[$payload]["1"]) {
    buffer[index++] = rest[$payload]["0"];
  }
  return buffer;
};
const $fs = {
  read_bytes: path => $fsRead(path, $fsByteList),
  write_bytes: request => $fsCall(request.path,
    fs => fs.writeFile(request.path, $fsByteBuffer(request.bytes))),
  append_bytes: request => $fsCall(request.path,
    fs => fs.appendFile(request.path, $fsByteBuffer(request.bytes))),
  read_text: path => $fsRead(path, $fsDecode),
  write_text: request => $fsCall(request.path,
    fs => fs.writeFile(request.path, $fsEncode(request.text))),
  append_text: request => $fsCall(request.path,
    fs => fs.appendFile(request.path, $fsEncode(request.text))),
  exists: path => $fsCall(path, async fs => {
    try {
      await fs.stat(path);
      return true;
    } catch (error) {
      if (error.code === "ENOENT") return false;
      throw error;
    }
  }),
  read_dir: path => $fsCall(path, async fs => {
    const entries = await fs.readdir(path, { withFileTypes: true, encoding: "buffer" });
    let result = $sum("None", undefined);
    for (let index = entries.length - 1; index >= 0; index--) {
      const entry = entries[index];
      let name;
      try {
        name = $fsDecode(entry.name);
      } catch (_) {
        throw $fsFailure("EINVAL", "Directory contains a name that is not valid UTF-8", path);
      }
      const value = $record([["name", name], ["kind", $fsKind(entry)]]);
      result = $sum("Cons", $record([["0", value], ["1", result]]));
    }
    return result;
  }),
  metadata: path => $fsCall(path, async fs => $fsMetadata(await fs.stat(path))),
  symlink_metadata: path => $fsCall(path, async fs => $fsMetadata(await fs.lstat(path))),
  create_dir: path => $fsCall(path, fs => fs.mkdir(path)),
  create_dir_all: path => $fsCall(path, async fs => { await fs.mkdir(path, { recursive: true }); }),
  remove_file: path => $fsCall(path, fs => fs.unlink(path)),
  remove_dir: path => $fsCall(path, fs => fs.rmdir(path)),
  rename: request => $fsCall(request.source,
    fs => fs.rename(request.source, $fsPath(request.destination))),
  copy_file: request => $fsCall(request.source,
    fs => fs.copyFile(request.source, $fsPath(request.destination)))
};
