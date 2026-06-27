import { createRouter, createWebHistory } from 'vue-router'
import { useAuth } from './store'

const routes = [
  { path: '/login', name: 'login', component: () => import('./views/Login.vue') },
  {
    path: '/',
    component: () => import('./components/Layout.vue'),
    children: [
      { path: '', redirect: '/dashboard' },
      { path: 'dashboard', name: 'dashboard', component: () => import('./views/Dashboard.vue') },
      { path: 'clients', name: 'clients', component: () => import('./views/Clients.vue') },
      { path: 'mappings', name: 'mappings', component: () => import('./views/Mappings.vue') },
      { path: 'logs', name: 'logs', component: () => import('./views/Logs.vue') },
      { path: 'settings', name: 'settings', component: () => import('./views/Settings.vue') }
    ]
  }
]

const router = createRouter({ history: createWebHistory(), routes })

router.beforeEach(async (to) => {
  const auth = useAuth()
  if (to.name === 'login') return true
  if (!auth.user) {
    await auth.fetchMe()
  }
  if (!auth.user) {
    return { name: 'login' }
  }
  return true
})

export default router
