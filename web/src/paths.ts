// How folders on the host are shown.

/** A folder with the home folder as `~`, on a Mac (/Users/name) and on Linux (/home/name, /root). */
export function shortPath(p: string): string {
  return p.replace(/^(\/(Users|home)\/[^/]+|\/root)(?=\/|$)/, '~');
}
