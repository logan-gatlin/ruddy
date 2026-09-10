const repository = "https://github.com/logan-gatlin/ruddy.git";
const gitPaths = new Set(["/info/refs", "/git-upload-pack"]);

export default {
  fetch(request, env) {
    const url = new URL(request.url);
    // Cargo's Git client adds an extra slash when the remote is a bare host.
    const pathname = url.pathname.replace(/^\/+/, "/");
    if (gitPaths.has(pathname)) {
      return Response.redirect(`${repository}${pathname}${url.search}`, 307);
    }
    return env.ASSETS.fetch(request);
  },
};
