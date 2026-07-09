import { createApp } from 'vue'
import { setup } from '@css-render/vue3-ssr'
import App from './App.vue'
import './style.css'

const app = createApp(App)

// Naive UI SSR setup (not needed for client-side, but prevents warnings)
setup(app)

app.mount('#app')
