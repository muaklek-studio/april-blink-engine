/// Marker for types that can be stored as components.
///
/// Implemented for every `'static` type; components are plain data and need no registration.
pub trait Component: 'static {}

impl<T: 'static> Component for T {}
