import { reactive } from 'vue'
import { api } from './api'

// Minimal reactive auth store (no Pinia needed).
const state = reactive({ user: null, loaded: false })

export function useAuth() {
  return {
    get user() {
      return state.user
    },
    async fetchMe() {
      try {
        state.user = await api.get('/api/auth/me')
      } catch {
        state.user = null
      }
      state.loaded = true
    },
    async login(username, password) {
      const res = await api.post('/api/auth/login', { username, password })
      state.user = { username: res.username }
      return res
    },
    async logout() {
      try {
        await api.post('/api/auth/logout', {})
      } catch {
        /* ignore */
      }
      state.user = null
    }
  }
}
