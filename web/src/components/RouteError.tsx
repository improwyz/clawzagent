import { isRouteErrorResponse, useRouteError, Link } from 'react-router-dom';

export function RouteError() {
  const error = useRouteError();
  let message = 'Unknown error';
  if (isRouteErrorResponse(error)) {
    message = `${error.status} ${error.statusText}`;
  } else if (error instanceof Error) {
    message = error.message;
  } else if (typeof error === 'string') {
    message = error;
  }

  return (
    <div className="flex h-screen flex-col items-center justify-center gap-3 bg-zinc-950 p-6 text-center">
      <h1 className="text-zinc-100 text-lg font-semibold">Page failed to load</h1>
      <p className="text-red-400 text-sm max-w-lg break-all">{message}</p>
      <Link to="/" className="text-blue-400 text-sm hover:underline">
        Back to dashboard
      </Link>
    </div>
  );
}
