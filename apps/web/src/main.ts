import { createApp } from 'vue'

import App from './App.vue'
import { i18n } from './i18n'
import { installTabsPersistence } from './services/tabsPersistence'
import './styles/tokens.css'
import './styles/menu.css'

installTabsPersistence()
createApp(App).use(i18n).mount('#app')
