//! Pilotage par d'autres programmes (voir « Pilotage par d'autres
//! programmes » dans le cahier des charges) : ce qui est commun à
//! l'interface, qui tient le canal de commande, et à ses clients (mode
//! commande, serveur MCP).
//!
//! JSON-RPC 2.0, un message JSON par ligne ([`protocol`]), commandes
//! décrites une seule fois ([`catalog`]). Le canal dépend du système : tube
//! nommé sous Windows (`polyptyque-windows`), socket Unix ailleurs
//! ([`unix`]), derrière les mêmes traits ([`transport`]).

pub mod catalog;
pub mod protocol;
pub mod transport;
#[cfg(unix)]
pub mod unix;

/// Argument de lancement de Polyptyque par le mode commande
/// (`polyptyque-cli app launch`) : directement en arrière-plan, sans ouvrir
/// les fenêtres de la scène, comme au démarrage de Windows.
pub const BACKGROUND_ARG: &str = "--arriere-plan";
