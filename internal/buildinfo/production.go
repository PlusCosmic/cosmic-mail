//go:build production

package buildinfo

// Debug is false in production builds: every test hook is compiled out.
const Debug = false
