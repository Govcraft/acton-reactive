/*
 * Copyright (c) 2024. Govcraft
 *
 * Licensed under either of
 *   * Apache License, Version 2.0 (the "License");
 *     you may not use this file except in compliance with the License.
 *     You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 *   * MIT license: http://opensource.org/licenses/MIT
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the applicable License for the specific language governing permissions and
 * limitations under that License.
 */
use std::fmt::{Display, Formatter};
use std::ops::{AddAssign, Div, Mul};
use std::ops::Deref;

// Magic Type ID - likely used for generating unique, content-addressable IDs.
use mti::prelude::*;

/// Represents an item within a shopping cart for the fruit market example.
#[derive(Default, Debug, Clone)]
pub(crate) struct CartItem {
    /// The name of the fruit item.
    name: String,
    /// The quantity of this item in the cart.
    quantity: i32,
    /// The cost per single unit of this item (in cents).
    cost: Cost,
    /// A unique identifier for the item type (e.g., based on the name).
    upc: MagicTypeId,
}


impl CartItem {
    /// Creates a new `CartItem` with a name and quantity.
    /// The cost defaults to 0 and the UPC is generated from the name.
    pub(crate) fn new(name: impl Into<String>, quantity: i32) -> Self {
        let name = name.into();
        let mut upc = "upc_".to_string();
        upc.push_str(&*name.clone());
        CartItem {
            name: name.into(),
            quantity,
            upc: upc.create_type_id::<V7>(),
            ..Default::default()
        }
    }

    /// Returns the unique identifier (UPC) of the cart item.
    pub(crate) fn id(&self) -> &MagicTypeId {
        &self.upc
    }

    /// Returns the name of the cart item.
    pub(crate) fn name(&self) -> &String {
        &self.name
    }

    /// Returns the quantity of the cart item.
    pub(crate) fn quantity(&self) -> i32 {
        self.quantity
    }

    /// Calculates and returns the total price for this cart item (cost * quantity).
    pub(crate) fn price(&self) -> Price {
        let price = &self.cost * self.quantity;
        Price(price)
    }
    /// Returns the cost per unit of the cart item.
    pub(crate) fn cost(&self) -> &Cost {
        &self.cost
    }

    /// Sets the quantity for this cart item.
    pub(crate) fn set_quantity(&mut self, quantity: i32) {
        self.quantity = quantity;
    }

    /// Sets the cost per unit for this cart item.
    pub(crate) fn set_cost(&mut self, cost: i32) {
        self.cost = Cost(cost);
    }
}

impl Display for CartItem {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Assuming `format_money` is a function that takes an `i32` and `Formatter`
        write!(f, "{} x {} @ {}", self.quantity, self.name, self.cost)
    }
}

/// Represents the cost of a single item, stored in cents.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cost(i32);

/// Allows treating `Cost` directly as an `i32` (its inner value).
impl Deref for Cost {
    type Target = i32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Implements display formatting for `Cost` using the `format_money` helper.
impl Display for Cost {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Assuming `format_money` is a function that takes an `i32` and `Formatter`
        format_money(self.0, f)
    }
}

/// Allows accessing the inner `i32` value via `as_ref()`.
impl AsRef<i32> for Cost {
    fn as_ref(&self) -> &i32 {
        &self.0
    }
}

/// Implements multiplication of a `Cost` reference by an `i32` quantity, returning the total price in cents.
impl Mul<i32> for &Cost {
    type Output = i32;

    fn mul(self, rhs: i32) -> Self::Output {
        self.0 * rhs
    }
}


/// Implements division of a `Cost` reference by an `i32`, returning a new `Cost` instance (e.g., for price adjustments).
impl Div<i32> for &Cost {
    type Output = Cost;

    fn div(self, rhs: i32) -> Self::Output {
        Cost(self.0 / rhs)
    }
}

/// Represents a total price, stored in cents.
#[derive(Default, Debug, Clone)]
pub(crate) struct Price(pub(crate) i32);

/// Implements display formatting for `Price` using the `format_money` helper.
impl Display for crate::cart_item::Price {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        format_money(self.0, f)
    }
}

/// Allows adding a `Price` directly to an `i32` total using `+=`.
impl AddAssign<Price> for i32 {
    fn add_assign(&mut self, rhs: Price) {
        *self += rhs.0;
    }
}


/// Helper function to format an integer representing cents into a $X.YY string format.
fn format_money(cents: i32, f: &mut Formatter) -> Result<(), std::fmt::Error> {
    write!(f, "${}.{}", cents / 100, cents % 100)
}
