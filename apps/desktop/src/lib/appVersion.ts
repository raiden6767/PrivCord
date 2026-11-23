import { useEffect, useState } from 'react';
import { config, isTauri } from '../config';

/** The version of the installed app. The page's own copy (package.json) is not the release number, so inside the app
 * it is read from the app itself; in a browser (development) the built-in one is the best there is. */
export function useAppVersion(): string {
  const [version, setVersion] = useState(config.appVersion);
  useEffect(() => {
    if (!isTauri()) return;
    let live = true;
    void import('@tauri-apps/api/app')
      .then(({ getVersion }) => getVersion())
      .then((v) => live && v && setVersion(v))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, []);
  return version;
}
