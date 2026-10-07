import { createRoot } from 'react-dom/client'
import { Console } from './Console'
import { bootConsole } from './boot'
import '../style.css'
import './console.css'
// Later fragment links cannot switch this tab to another driver's credential.
window.addEventListener('hashchange', () => {
  const url = new URL(location.href),
    fragment = new URLSearchParams(url.hash.slice(1))
  if (fragment.has('cap')) {
    fragment.delete('cap')
    url.hash = fragment.toString()
    history.replaceState(null, '', url)
  }
})
try {
  const boot = bootConsole()
  createRoot(document.getElementById('root')!).render(<Console boot={boot} />)
} catch (error) {
  createRoot(document.getElementById('root')!).render(
    <main className="human-console">
      <h1>Seat console</h1>
      <p role="alert">
        {error instanceof Error ? error.message : 'Cannot open seat access'}
      </p>
    </main>,
  )
}
